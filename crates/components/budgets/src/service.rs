//! The budgets service: holds the pool and the injected [`Clock`]. It creates budgets and
//! projects, places citizen orders **under the ceiling**, and reads them back with keyset
//! pagination. All failures map onto the canonical [`dsoc_core::Error`].
//!
//! This domain emits **no** cross-crate events: `budgets.*` is not part of the frozen
//! [`dsoc_core::events::Event`] catalog and nothing consumes it, so (like `dsoc-admin` and
//! `dsoc-debates`) the crate keeps its state private and exposes it only through its routes.
//! Time is read only from the injected [`Clock`], never ambiently (TESTING.md).
//!
//! There is no `BudgetId`/`OrderId` in the frozen `dsoc_core::ids`, and those ids never cross
//! a crate boundary (no peer consumes them), so budget/project/order identity is a bare
//! `uuid::Uuid` here; the cross-boundary identities (`org`, `citizen`) remain typed newtypes.

use std::collections::BTreeSet;
use std::sync::Arc;

use dsoc_core::ids::{CitizenId, OrgId};
use dsoc_core::{Clock, Error, Result};
use dsoc_db::Db;
use uuid::Uuid;

use crate::domain::{self, NewBudget, NewProject};
use crate::queries::{self, BudgetRow, OrderRow, ProjectRow};

/// Default page size for listings.
pub const DEFAULT_PAGE_LIMIT: i64 = 20;
/// Hard cap on page size to keep reads bounded.
pub const MAX_PAGE_LIMIT: i64 = 100;

/// A placed order: the stored header, the total it allocated (cents), and the project ids it
/// selected (ascending). The total is recomputed from the selected projects, never trusted
/// from the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedOrder {
    /// The stored order header.
    pub order: OrderRow,
    /// Total allocated by this order, in cents (`<= budget.ceiling_cents`).
    pub allocated_cents: i64,
    /// The project ids this order allocated to, ascending.
    pub project_ids: Vec<Uuid>,
}

/// Participatory-budgeting service. Cheap to construct per request from `AppState`.
#[derive(Clone)]
pub struct BudgetService {
    db: Db,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for BudgetService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BudgetService").finish_non_exhaustive()
    }
}

impl BudgetService {
    /// Build a service from an explicit pool and clock (used by tests).
    #[must_use]
    pub fn new(db: Db, clock: Arc<dyn Clock>) -> Self {
        Self { db, clock }
    }

    /// Build a service from the shared application state.
    #[must_use]
    pub fn from_state(state: &dsoc_app::AppState) -> Self {
        Self::new(state.db.clone(), state.clock.clone())
    }

    /// Create a budget owned by `org`. Stamped with the injected clock's time.
    ///
    /// # Errors
    /// - [`Error::Conflict`] on a unique violation.
    /// - [`Error::Validation`] on a foreign-key/check violation (e.g. unknown org).
    /// - [`Error::Storage`] on any other persistence failure.
    pub async fn create_budget(&self, org: OrgId, new: &NewBudget) -> Result<BudgetRow> {
        queries::insert_budget(
            &self.db,
            Uuid::now_v7(),
            org.as_uuid(),
            &new.title,
            new.ceiling_cents,
            self.clock.now(),
        )
        .await
        .map_err(map_sqlx)
    }

    /// Fetch a single budget.
    ///
    /// # Errors
    /// [`Error::NotFound`] when absent; [`Error::Storage`] otherwise.
    pub async fn get_budget(&self, budget: Uuid) -> Result<BudgetRow> {
        queries::get_budget(&self.db, budget)
            .await
            .map_err(map_sqlx)
    }

    /// List an org's budgets with keyset pagination, returning the rows and the total count.
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn list_budgets(
        &self,
        org: OrgId,
        after: Option<Uuid>,
        limit: i64,
    ) -> Result<(Vec<BudgetRow>, i64)> {
        let rows = queries::list_budgets(&self.db, org.as_uuid(), after, clamp_limit(limit))
            .await
            .map_err(map_sqlx)?;
        let total = queries::count_budgets(&self.db, org.as_uuid())
            .await
            .map_err(map_sqlx)?;
        Ok((rows, total))
    }

    /// Add a project to a budget. The budget must exist (a missing budget is a clean
    /// [`Error::NotFound`] for the path id, never a raw FK error).
    ///
    /// # Errors
    /// - [`Error::NotFound`] when the budget does not exist.
    /// - [`Error::Validation`] on a foreign-key/check violation.
    /// - [`Error::Storage`] on any other persistence failure.
    pub async fn add_project(&self, budget: Uuid, new: &NewProject) -> Result<ProjectRow> {
        // Confirm the budget exists so a missing path id is a 404, not a generic FK error.
        self.get_budget(budget).await?;
        queries::insert_project(
            &self.db,
            Uuid::now_v7(),
            budget,
            &new.title,
            new.cost_cents,
            self.clock.now(),
        )
        .await
        .map_err(map_sqlx)
    }

    /// List a budget's projects with keyset pagination, returning the rows and total.
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn list_projects(
        &self,
        budget: Uuid,
        after: Option<Uuid>,
        limit: i64,
    ) -> Result<(Vec<ProjectRow>, i64)> {
        let rows = queries::list_projects(&self.db, budget, after, clamp_limit(limit))
            .await
            .map_err(map_sqlx)?;
        let total = queries::count_projects(&self.db, budget)
            .await
            .map_err(map_sqlx)?;
        Ok((rows, total))
    }

    /// Place a `citizen`'s order against a budget, allocating to the given projects. The
    /// allocation rule ([`domain::check_allocation`]) is enforced **server-side** over the
    /// stored project costs (never a client-supplied total): the sum must not exceed the
    /// budget ceiling. The order header and its items are written in one transaction so a
    /// partial order can never be observed.
    ///
    /// Duplicate `project_ids` are de-duplicated. A citizen has at most one order per budget
    /// (guarded by a UNIQUE constraint); a second attempt is an [`Error::Conflict`].
    ///
    /// # Errors
    /// - [`Error::NotFound`] when the budget does not exist.
    /// - [`Error::Validation`] when no projects are selected, or a selected project does not
    ///   belong to this budget.
    /// - [`Error::Conflict`] when the allocation exceeds the ceiling, or the citizen already
    ///   ordered against this budget.
    /// - [`Error::Storage`] on any other persistence failure.
    pub async fn place_order(
        &self,
        budget: Uuid,
        citizen: CitizenId,
        project_ids: &[Uuid],
    ) -> Result<PlacedOrder> {
        // De-duplicate while keeping a stable ascending order (BTreeSet) for items + asserts.
        let unique: Vec<Uuid> = project_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if unique.is_empty() {
            return Err(Error::Validation(
                "an order must allocate to at least one project".to_string(),
            ));
        }

        let mut tx = self.db.begin().await.map_err(map_sqlx)?;

        // Budget must exist: a missing budget is a clean 404 for the path id.
        let budget_row = queries::get_budget(&mut *tx, budget)
            .await
            .map_err(map_sqlx)?;

        // Resolve the selected projects' costs, scoped to this budget. Any id not returned is
        // unknown or belongs to another budget -> a clean Validation error.
        let found = queries::project_costs_in_budget(&mut *tx, budget, &unique)
            .await
            .map_err(map_sqlx)?;
        if found.len() != unique.len() {
            return Err(Error::Validation(
                "an order may only allocate to projects of this budget".to_string(),
            ));
        }

        // Enforce the ceiling over the server-side costs (never a client-supplied total).
        let costs: Vec<i64> = found.iter().map(|(_, cost)| *cost).collect();
        let allocated_cents = domain::check_allocation(&costs, budget_row.ceiling_cents)?;

        let order = queries::insert_order(
            &mut *tx,
            Uuid::now_v7(),
            budget,
            citizen.as_uuid(),
            self.clock.now(),
        )
        .await
        .map_err(map_sqlx)?;

        queries::insert_order_items(&mut *tx, order.id, &unique)
            .await
            .map_err(map_sqlx)?;

        tx.commit().await.map_err(map_sqlx)?;

        Ok(PlacedOrder {
            order,
            allocated_cents,
            project_ids: unique,
        })
    }

    /// List a budget's orders with keyset pagination, returning the rows and total.
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn list_orders(
        &self,
        budget: Uuid,
        after: Option<Uuid>,
        limit: i64,
    ) -> Result<(Vec<OrderRow>, i64)> {
        let rows = queries::list_orders(&self.db, budget, after, clamp_limit(limit))
            .await
            .map_err(map_sqlx)?;
        let total = queries::count_orders(&self.db, budget)
            .await
            .map_err(map_sqlx)?;
        Ok((rows, total))
    }

    /// Read back the project ids an order allocated to (ascending).
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn order_project_ids(&self, order: Uuid) -> Result<Vec<Uuid>> {
        queries::order_item_project_ids(&self.db, order)
            .await
            .map_err(map_sqlx)
    }
}

/// Clamp a requested page size into `1..=MAX_PAGE_LIMIT`.
fn clamp_limit(limit: i64) -> i64 {
    domain::clamp_limit(limit, MAX_PAGE_LIMIT)
}

/// Map a `sqlx` failure onto the canonical error model (CONTRIBUTING.md wiring conventions):
/// missing row -> `NotFound`, unique violation -> `Conflict`, foreign-key/check violation ->
/// `Validation`, everything else -> `Storage` (logged server-side, never surfaced raw).
fn map_sqlx(err: sqlx::Error) -> Error {
    match err {
        sqlx::Error::RowNotFound => Error::NotFound("budget not found".to_string()),
        sqlx::Error::Database(ref db) if db.is_unique_violation() => {
            Error::Conflict("this citizen has already ordered against this budget".to_string())
        }
        sqlx::Error::Database(ref db) if db.is_foreign_key_violation() => Error::Validation(
            "referenced org, budget, project or citizen does not exist".to_string(),
        ),
        sqlx::Error::Database(ref db) if db.is_check_violation() => {
            Error::Validation("input violates a constraint".to_string())
        }
        other => Error::Storage(Box::new(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_limit_bounds_page_size() {
        assert_eq!(clamp_limit(0), 1);
        assert_eq!(clamp_limit(-10), 1);
        assert_eq!(clamp_limit(20), 20);
        assert_eq!(clamp_limit(10_000), MAX_PAGE_LIMIT);
    }

    #[test]
    fn map_sqlx_row_not_found_is_not_found() {
        assert_eq!(map_sqlx(sqlx::Error::RowNotFound).code(), "not_found");
    }

    #[test]
    fn map_sqlx_other_is_storage() {
        assert_eq!(map_sqlx(sqlx::Error::PoolTimedOut).code(), "storage_error");
    }
}

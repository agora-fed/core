//! The debates service: holds the pool and the injected [`Clock`]. It creates debates and
//! contributions and reads them back with keyset pagination. All failures map onto the
//! canonical [`dsoc_core::Error`].
//!
//! This domain emits **no** cross-crate events: `debates.*` is not part of the frozen
//! [`dsoc_core::events::Event`] catalog and nothing consumes it, so (like `dsoc-admin`) the
//! crate keeps its state private and exposes it only through its routes. Time is read only
//! from the injected [`Clock`], never ambiently (TESTING.md).
//!
//! There is no `DebateId` in the frozen `dsoc_core::ids`, and a debate id never crosses a
//! crate boundary (no peer consumes it), so the debate identity is a bare `uuid::Uuid`
//! here; the cross-boundary identities (`org`, `author`) remain the typed core newtypes.

use std::sync::Arc;

use dsoc_core::ids::{CitizenId, OrgId};
use dsoc_core::{Clock, Error, Result};
use dsoc_db::Db;
use uuid::Uuid;

use crate::domain::{NewContribution, NewDebate};
use crate::queries::{self, ContributionRow, DebateRow};

/// Default page size for listings.
pub const DEFAULT_PAGE_LIMIT: i64 = 20;
/// Hard cap on page size to keep reads bounded.
pub const MAX_PAGE_LIMIT: i64 = 100;

/// Structured-debates service. Cheap to construct per request from `AppState`.
#[derive(Clone)]
pub struct DebateService {
    db: Db,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for DebateService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DebateService").finish_non_exhaustive()
    }
}

impl DebateService {
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

    /// Create a debate owned by `org`. Stamped with the injected clock's time.
    ///
    /// # Errors
    /// - [`Error::Conflict`] on a unique violation.
    /// - [`Error::Validation`] on a foreign-key/check violation (e.g. unknown org).
    /// - [`Error::Storage`] on any other persistence failure.
    pub async fn create_debate(&self, org: OrgId, new: &NewDebate) -> Result<DebateRow> {
        queries::insert_debate(
            &self.db,
            Uuid::now_v7(),
            org.as_uuid(),
            &new.title,
            &new.framing,
            new.uf.as_deref(),
            self.clock.now(),
        )
        .await
        .map_err(map_sqlx)
    }

    /// Fetch a single debate.
    ///
    /// # Errors
    /// [`Error::NotFound`] when absent; [`Error::Storage`] otherwise.
    pub async fn get_debate(&self, debate: Uuid) -> Result<DebateRow> {
        queries::get_debate(&self.db, debate)
            .await
            .map_err(map_sqlx)
    }

    /// List an org's debates with keyset pagination, returning the rows and the total count.
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn list_debates(
        &self,
        org: OrgId,
        after: Option<Uuid>,
        limit: i64,
    ) -> Result<(Vec<DebateRow>, i64)> {
        let rows = queries::list_debates(&self.db, org.as_uuid(), after, clamp_limit(limit))
            .await
            .map_err(map_sqlx)?;
        let total = queries::count_debates(&self.db, org.as_uuid())
            .await
            .map_err(map_sqlx)?;
        Ok((rows, total))
    }

    /// Contribute to a debate. The debate must exist (a missing debate is a clean
    /// [`Error::NotFound`] for the path id, never a raw FK error); the validated stance and
    /// body are then inserted, stamped with the injected clock's time and the authenticated
    /// `author`.
    ///
    /// # Errors
    /// - [`Error::NotFound`] when the debate does not exist.
    /// - [`Error::Conflict`] on a unique violation.
    /// - [`Error::Validation`] on a foreign-key/check violation (e.g. unknown author).
    /// - [`Error::Storage`] on any other persistence failure.
    pub async fn contribute(
        &self,
        debate: Uuid,
        author: CitizenId,
        new: &NewContribution,
    ) -> Result<ContributionRow> {
        // Confirm the debate exists so a missing path id is a 404, not a generic FK error.
        self.get_debate(debate).await?;
        queries::insert_contribution(
            &self.db,
            Uuid::now_v7(),
            debate,
            author.as_uuid(),
            new.stance.as_str(),
            &new.body,
            self.clock.now(),
        )
        .await
        .map_err(map_sqlx)
    }

    /// List a debate's contributions with keyset pagination, returning the rows and total.
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn list_contributions(
        &self,
        debate: Uuid,
        after: Option<Uuid>,
        limit: i64,
    ) -> Result<(Vec<ContributionRow>, i64)> {
        let rows = queries::list_contributions(&self.db, debate, after, clamp_limit(limit))
            .await
            .map_err(map_sqlx)?;
        let total = queries::count_contributions(&self.db, debate)
            .await
            .map_err(map_sqlx)?;
        Ok((rows, total))
    }
}

/// Clamp a requested page size into `1..=MAX_PAGE_LIMIT`.
fn clamp_limit(limit: i64) -> i64 {
    limit.clamp(1, MAX_PAGE_LIMIT)
}

/// Map a `sqlx` failure onto the canonical error model (CONTRIBUTING.md wiring conventions):
/// missing row -> `NotFound`, unique violation -> `Conflict`, foreign-key/check violation ->
/// `Validation`, everything else -> `Storage` (logged server-side, never surfaced raw).
fn map_sqlx(err: sqlx::Error) -> Error {
    match err {
        sqlx::Error::RowNotFound => Error::NotFound("debate not found".to_string()),
        sqlx::Error::Database(ref db) if db.is_unique_violation() => {
            Error::Conflict("debate already exists".to_string())
        }
        sqlx::Error::Database(ref db) if db.is_foreign_key_violation() => {
            Error::Validation("referenced org, debate or citizen does not exist".to_string())
        }
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

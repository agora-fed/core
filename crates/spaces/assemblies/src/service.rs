//! `AssembliesService` — the concrete assembly-membership service. Holds the `Db` pool plus the
//! injected `Arc<dyn Clock>` and `Arc<dyn Authorization>` (ADR-0004 wiring). It creates permanent
//! participatory bodies and manages their rosters.
//!
//! Every mutation authorizes the AUTHENTICATED caller via the injected [`Authorization`] (never a
//! body-supplied id), takes its time from the injected [`Clock`] (never the ambient wall clock),
//! and maps every `sqlx` failure onto the canonical [`dsoc_core::Error`] model. Adding a member is
//! idempotent: the `(assembly_id, citizen_id)` UNIQUE key collapses a re-add to the existing row,
//! whose REAL stored id is returned (RETURNING on insert, re-select on conflict).
//!
//! An assembly's id doubles as the logical [`SpaceId`] of its participation space, so the public
//! surface speaks in `SpaceId` (no separate spaces table exists).

use std::sync::Arc;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use dsoc_app::AppState;
use dsoc_core::ids::{CitizenId, OrgId, SpaceId};
use dsoc_core::{Authorization, Clock, Error, Result};
use dsoc_db::Db;

use crate::domain::{self, MIN_MUTATION_LEVEL};
use crate::queries::{self, AssemblyRow, MemberRow};

/// Default page size for list endpoints when the caller does not specify one.
pub const DEFAULT_PAGE_LIMIT: u32 = 50;

/// Hard cap on a list page, applied even if the caller requests more (unbounded reads must be
/// bounded — PLAN.md).
pub const MAX_PAGE_LIMIT: u32 = 100;

/// A permanent participatory body.
#[derive(Debug, Clone)]
pub struct Assembly {
    /// The assembly id (doubles as the logical [`SpaceId`]).
    pub id: SpaceId,
    /// The organization that owns the body.
    pub org: OrgId,
    /// Public display name.
    pub name: String,
    /// When the body was created (injected clock).
    pub created_at: DateTime<Utc>,
}

/// A roster membership binding a citizen to an assembly with a role.
#[derive(Debug, Clone)]
pub struct Member {
    /// Real stored membership id (RETURNING on insert / re-selected on idempotent conflict).
    pub id: Uuid,
    /// The assembly the citizen belongs to.
    pub assembly: SpaceId,
    /// The member citizen.
    pub citizen: CitizenId,
    /// The member's role within the body (canonical stored form).
    pub role: String,
    /// When the citizen joined (injected clock).
    pub joined_at: DateTime<Utc>,
}

/// The assembly & membership service.
#[derive(Clone)]
pub struct AssembliesService {
    db: Db,
    clock: Arc<dyn Clock>,
    authz: Arc<dyn Authorization>,
}

impl std::fmt::Debug for AssembliesService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AssembliesService")
            .field("db", &"PgPool")
            .finish_non_exhaustive()
    }
}

/// Map a `sqlx` failure onto the canonical, public-safe error model.
fn map_sqlx(error: sqlx::Error) -> Error {
    match error {
        sqlx::Error::RowNotFound => Error::NotFound("entity not found".to_owned()),
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            Error::Conflict("entity already exists".to_owned())
        }
        other => Error::Storage(Box::new(other)),
    }
}

/// Clamp a caller-supplied page size into `[1, MAX_PAGE_LIMIT]`, defaulting when absent.
fn clamp_limit(limit: Option<u32>) -> i64 {
    i64::from(limit.unwrap_or(DEFAULT_PAGE_LIMIT).clamp(1, MAX_PAGE_LIMIT))
}

impl AssembliesService {
    /// Construct from explicitly injected ports (used directly by integration tests).
    #[must_use]
    pub fn new(db: Db, clock: Arc<dyn Clock>, authz: Arc<dyn Authorization>) -> Self {
        Self { db, clock, authz }
    }

    /// Construct from the shared [`AppState`] the gateway injects (ADR-0004 wiring). The event bus
    /// is intentionally unused: this space emits no cross-crate events (its membership is internal
    /// state), so it never publishes.
    #[must_use]
    pub fn from_state(state: &AppState) -> Self {
        Self {
            db: state.db.clone(),
            clock: Arc::clone(&state.clock),
            authz: Arc::clone(&state.authz),
        }
    }

    /// Assert the caller may perform an assembly mutation in `org` (never trusts a body-supplied
    /// id; it checks the AUTHENTICATED caller against the injected [`Authorization`]).
    async fn authorize_mutation(&self, org: OrgId, caller: CitizenId) -> Result<()> {
        self.authz.require(org, caller, MIN_MUTATION_LEVEL).await
    }

    /// Create a permanent participatory body in `org`. The acting caller is authorized first; the
    /// name is validated at the boundary. Returns the REAL stored row.
    ///
    /// # Errors
    /// [`Error::Forbidden`] if the caller is unauthorized; [`Error::Validation`] on a blank name;
    /// [`Error::Storage`]/[`Error::Conflict`] on persistence failure.
    pub async fn create_assembly(
        &self,
        org: OrgId,
        caller: CitizenId,
        name: &str,
    ) -> Result<Assembly> {
        self.authorize_mutation(org, caller).await?;
        let name = domain::validate_text("name", name)?;
        let now = self.clock.now();

        let row = queries::insert_assembly(&self.db, Uuid::now_v7(), org.as_uuid(), &name, now)
            .await
            .map_err(map_sqlx)?;
        Ok(assembly_from_row(row))
    }

    /// Fetch an assembly by id within an organization (read-only).
    ///
    /// # Errors
    /// [`Error::NotFound`] if absent; [`Error::Storage`] on persistence failure.
    pub async fn get_assembly(&self, org: OrgId, assembly: SpaceId) -> Result<Assembly> {
        let row = queries::find_assembly(&self.db, org.as_uuid(), assembly.as_uuid())
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| Error::NotFound("assembly not found".to_owned()))?;
        Ok(assembly_from_row(row))
    }

    /// Add a citizen to an assembly's roster with `role`. The acting caller is authorized first;
    /// the role is normalized/validated. Idempotent: re-adding the same citizen returns the
    /// existing membership (its real stored id), never a duplicate and never a role change.
    ///
    /// # Errors
    /// [`Error::Forbidden`] if unauthorized; [`Error::Validation`] on an unknown role;
    /// [`Error::NotFound`] if the assembly does not exist in `org`; [`Error::Storage`] on
    /// persistence failure.
    pub async fn add_member(
        &self,
        org: OrgId,
        caller: CitizenId,
        assembly: SpaceId,
        citizen: CitizenId,
        role: &str,
    ) -> Result<Member> {
        self.authorize_mutation(org, caller).await?;
        let role = domain::normalize_role(role)?;
        let now = self.clock.now();

        let mut tx = self.db.begin().await.map_err(map_sqlx)?;

        // The assembly must exist in this org before we touch its roster (a wrong id would
        // otherwise surface only as an opaque FK storage error).
        queries::find_assembly(&mut *tx, org.as_uuid(), assembly.as_uuid())
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| Error::NotFound("assembly not found".to_owned()))?;

        // Idempotent insert: a fresh row is returned directly; on the UNIQUE conflict we re-select
        // the existing membership so the caller always gets the REAL stored id.
        let row = match queries::insert_member(
            &mut *tx,
            Uuid::now_v7(),
            assembly.as_uuid(),
            citizen.as_uuid(),
            &role,
            now,
            now,
        )
        .await
        .map_err(map_sqlx)?
        {
            Some(row) => row,
            None => queries::find_member(&mut *tx, assembly.as_uuid(), citizen.as_uuid())
                .await
                .map_err(map_sqlx)?
                .ok_or_else(|| Error::Storage(Box::new(sqlx::Error::RowNotFound)))?,
        };

        tx.commit().await.map_err(map_sqlx)?;
        Ok(member_from_row(row))
    }

    /// Keyset-paginated roster for an assembly (ordered by member id ascending).
    ///
    /// # Errors
    /// [`Error::Storage`] on persistence failure.
    pub async fn list_members(
        &self,
        assembly: SpaceId,
        after: Option<Uuid>,
        limit: Option<u32>,
    ) -> Result<Vec<Member>> {
        let rows = queries::list_members(&self.db, assembly.as_uuid(), after, clamp_limit(limit))
            .await
            .map_err(map_sqlx)?;
        Ok(rows.into_iter().map(member_from_row).collect())
    }
}

/// Map an assembly row to its public domain type.
fn assembly_from_row(row: AssemblyRow) -> Assembly {
    Assembly {
        id: SpaceId::from_uuid(row.id),
        org: OrgId::from_uuid(row.org_id),
        name: row.name,
        created_at: row.created_at,
    }
}

/// Map a roster row to its public domain type.
fn member_from_row(row: MemberRow) -> Member {
    Member {
        id: row.id,
        assembly: SpaceId::from_uuid(row.assembly_id),
        citizen: CitizenId::from_uuid(row.citizen_id),
        role: row.role,
        joined_at: row.joined_at,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn map_sqlx_row_not_found_is_not_found() {
        assert_eq!(map_sqlx(sqlx::Error::RowNotFound).code(), "not_found");
    }

    #[test]
    fn map_sqlx_protocol_error_is_storage_and_hides_detail() {
        let mapped = map_sqlx(sqlx::Error::Protocol("secret".into()));
        assert_eq!(mapped.code(), "storage_error");
        assert_eq!(mapped.to_string(), "storage error");
    }

    #[test]
    fn clamp_limit_defaults_and_bounds() {
        assert_eq!(clamp_limit(None), i64::from(DEFAULT_PAGE_LIMIT));
        assert_eq!(clamp_limit(Some(0)), 1);
        assert_eq!(clamp_limit(Some(10)), 10);
        assert_eq!(clamp_limit(Some(10_000)), i64::from(MAX_PAGE_LIMIT));
    }
}

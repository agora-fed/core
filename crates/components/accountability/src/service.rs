//! The accountability service: create tracked commitments and append authorized progress updates.
//!
//! Writes are authorized actions. Every mutation AUTHORIZES the authenticated caller against the
//! relevant org via the injected [`Authorization`] port BEFORE any write — for a status update the
//! org is the result's REAL org (read from storage), never an org taken from the request body
//! (SECURITY.md / ADR-0007). Posting an update appends to the ledger AND recomputes the parent
//! result's denormalised `progress`/`status` in the SAME transaction, and the progress transition
//! is guarded by the expected prior state (`status <> 'achieved'`) so a terminal result can never be
//! reopened and concurrent writers cannot race past the `achieved` boundary.

use std::sync::Arc;

use dsoc_core::ids::{CitizenId, OrgId};
use dsoc_core::{Authorization, Clock, Error, Result, VerificationLevel};
use dsoc_db::Db;
use uuid::Uuid;

use crate::domain::{self, Commitment, Status, StatusUpdate};
use crate::queries::{self, Cursor};

/// Upper bound on a result's title.
const MAX_TITLE_LEN: usize = 200;
/// Upper bound on a result's target text.
const MAX_TARGET_LEN: usize = 2_000;
/// Upper bound on a status-update note.
const MAX_NOTE_LEN: usize = 2_000;
/// Default page size for list endpoints.
pub const DEFAULT_PAGE_LIMIT: i64 = 50;
/// Hard cap on page size to keep reads bounded.
pub const MAX_PAGE_LIMIT: i64 = 200;
/// Minimum verification a caller needs to record results/progress. Accountability records are
/// authoritative, so they require a directory-verified author (PLAN.md `mandates`).
const MIN_AUTHOR_LEVEL: VerificationLevel = VerificationLevel::Directory;

/// The accountability service. Cheap to construct per request from `AppState`.
pub struct AccountabilityService {
    db: Db,
    clock: Arc<dyn Clock>,
    authz: Arc<dyn Authorization>,
}

impl std::fmt::Debug for AccountabilityService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountabilityService")
            .finish_non_exhaustive()
    }
}

impl AccountabilityService {
    /// Build a service from its injected ports.
    #[must_use]
    pub fn new(db: Db, clock: Arc<dyn Clock>, authz: Arc<dyn Authorization>) -> Self {
        Self { db, clock, authz }
    }

    /// Build a service from the shared application state.
    #[must_use]
    pub fn from_state(state: &dsoc_app::AppState) -> Self {
        Self::new(state.db.clone(), state.clock.clone(), state.authz.clone())
    }

    // --- authorized mutations ----------------------------------------------------------------

    /// Create a tracked commitment owned by `org`, in its initial (`pending`, progress 0) state.
    /// AUTHORIZES the authenticated `actor` against `org` at directory level before writing.
    ///
    /// # Errors
    /// [`Error::Unauthorized`]/[`Error::Forbidden`] when the actor is insufficiently verified;
    /// [`Error::Validation`] for empty/oversized title or oversized target; [`Error::Storage`]
    /// otherwise.
    pub async fn create_result(
        &self,
        actor: CitizenId,
        org: OrgId,
        title: &str,
        target: &str,
    ) -> Result<Commitment> {
        self.authz.require(org, actor, MIN_AUTHOR_LEVEL).await?;

        let title = domain::validate_nonempty("title", title, MAX_TITLE_LEN).map_err(validation)?;
        let target =
            domain::validate_bounded("target", target, MAX_TARGET_LEN).map_err(validation)?;
        let now = self.clock.now();
        let id = Uuid::now_v7();
        let stored_id = queries::insert_result(&self.db, id, org.as_uuid(), &title, &target, now)
            .await
            .map_err(map_storage)?;
        // Read back the real stored row so the response reflects committed state.
        queries::get_result(&self.db, stored_id)
            .await
            .map_err(map_storage)
    }

    /// Post a progress update against a result. AUTHORIZES the authenticated `actor` against the
    /// result's REAL org (derived from storage — never the caller's input) at directory level, then
    /// appends the ledger row and recomputes the result's denormalised `progress`/`status` in ONE
    /// transaction. The transition is guarded by the expected prior state: a result already at its
    /// terminal `achieved` status rejects further updates with [`Error::Conflict`].
    ///
    /// Returns the recomputed result alongside the persisted update.
    ///
    /// # Errors
    /// [`Error::NotFound`] when the result does not exist; [`Error::Unauthorized`]/
    /// [`Error::Forbidden`] when the actor is insufficiently verified; [`Error::Validation`] for an
    /// empty/oversized note or out-of-range progress; [`Error::Conflict`] when the result is already
    /// terminal; [`Error::Storage`] otherwise.
    pub async fn post_status_update(
        &self,
        actor: CitizenId,
        result_id: Uuid,
        note: &str,
        progress: i32,
    ) -> Result<(Commitment, StatusUpdate)> {
        let result = queries::get_result(&self.db, result_id)
            .await
            .map_err(map_storage)?;
        // Authorize against the resource's actual org, derived from storage — not the caller's input.
        self.authz
            .require(OrgId::from_uuid(result.org_id), actor, MIN_AUTHOR_LEVEL)
            .await?;

        let note = domain::validate_nonempty("note", note, MAX_NOTE_LEN).map_err(validation)?;
        let progress = domain::validate_progress(progress).map_err(validation)?;

        if result.status.is_terminal() {
            return Err(Error::Conflict(
                "result is already achieved and cannot be updated".to_owned(),
            ));
        }

        let derived = Status::from_progress(progress);
        let now = self.clock.now();
        let mut tx = self.db.begin().await.map_err(map_sqlx)?;

        // Guarded transition: only moves a non-terminal result; None means a concurrent writer
        // already drove it to `achieved` (TOCTOU) — surface that as a conflict.
        let updated = queries::apply_progress(&mut *tx, result_id, progress, derived)
            .await
            .map_err(map_storage)?
            .ok_or_else(|| {
                Error::Conflict("result is already achieved and cannot be updated".to_owned())
            })?;

        let update = StatusUpdate {
            id: Uuid::now_v7(),
            result_id,
            note,
            progress,
            created_at: now,
        };
        queries::insert_status_update(&mut *tx, &update)
            .await
            .map_err(map_storage)?;

        tx.commit().await.map_err(map_sqlx)?;
        Ok((updated, update))
    }

    // --- public reads ------------------------------------------------------------------------

    /// Fetch a single tracked commitment by id.
    ///
    /// # Errors
    /// [`Error::NotFound`] when absent; [`Error::Storage`] otherwise.
    pub async fn get_result(&self, id: Uuid) -> Result<Commitment> {
        queries::get_result(&self.db, id).await.map_err(map_storage)
    }

    /// List an org's tracked commitments newest-first (keyset-paginated).
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn list_results(
        &self,
        org: OrgId,
        cursor: Option<Cursor>,
        limit: i64,
    ) -> Result<Vec<Commitment>> {
        queries::list_results(&self.db, org.as_uuid(), cursor, clamp_limit(limit))
            .await
            .map_err(map_storage)
    }

    /// List a result's progress updates newest-first (keyset-paginated).
    ///
    /// # Errors
    /// [`Error::NotFound`] when the result does not exist; [`Error::Storage`] otherwise.
    pub async fn list_status_updates(
        &self,
        result_id: Uuid,
        cursor: Option<Cursor>,
        limit: i64,
    ) -> Result<Vec<StatusUpdate>> {
        // Confirm the result exists so an unknown id is a clean NotFound, not an empty page.
        queries::get_result(&self.db, result_id)
            .await
            .map_err(map_storage)?;
        queries::list_status_updates(&self.db, result_id, cursor, clamp_limit(limit))
            .await
            .map_err(map_storage)
    }
}

fn clamp_limit(limit: i64) -> i64 {
    limit.clamp(1, MAX_PAGE_LIMIT)
}

fn validation(err: domain::ParseError) -> Error {
    Error::Validation(err.to_string())
}

/// Map a raw `sqlx` failure (from transaction calls) onto the canonical error model.
fn map_sqlx(err: sqlx::Error) -> Error {
    match err {
        sqlx::Error::RowNotFound => Error::NotFound("accountability entity not found".to_owned()),
        sqlx::Error::Database(ref db) if db.is_unique_violation() => {
            Error::Conflict("accountability entity already exists".to_owned())
        }
        other => Error::Storage(Box::new(other)),
    }
}

/// Map a storage-layer [`queries::Error`] onto the canonical [`dsoc_core::Error`]
/// (`RowNotFound` -> `NotFound`; unique -> `Conflict`; FK/check/not-null -> `Validation`; corrupt
/// stored value -> `Storage`; everything else -> `Storage`).
fn map_storage(err: queries::Error) -> Error {
    match err {
        queries::Error::Db(sqlx::Error::RowNotFound) => {
            Error::NotFound("accountability entity not found".to_owned())
        }
        queries::Error::Db(sqlx::Error::Database(db_err)) => match db_err.code().as_deref() {
            Some("23505") => Error::Conflict("accountability entity already exists".to_owned()),
            Some("23503" | "23514" | "23502") => {
                Error::Validation("accountability input violates a constraint".to_owned())
            }
            _ => Error::Storage(Box::new(db_err)),
        },
        queries::Error::Db(other) => Error::Storage(Box::new(other)),
        queries::Error::Decode(parse) => Error::Storage(Box::new(parse)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_limit_bounds_page_size() {
        assert_eq!(clamp_limit(0), 1);
        assert_eq!(clamp_limit(-5), 1);
        assert_eq!(clamp_limit(50), 50);
        assert_eq!(clamp_limit(10_000), MAX_PAGE_LIMIT);
    }

    #[test]
    fn author_level_is_directory() {
        assert_eq!(MIN_AUTHOR_LEVEL, VerificationLevel::Directory);
    }

    #[test]
    fn row_not_found_maps_to_not_found() {
        let mapped = map_storage(queries::Error::Db(sqlx::Error::RowNotFound));
        assert!(matches!(mapped, Error::NotFound(_)));
        assert_eq!(mapped.code(), "not_found");
    }

    #[test]
    fn sqlx_row_not_found_and_pool_timeout_map_correctly() {
        assert_eq!(map_sqlx(sqlx::Error::RowNotFound).code(), "not_found");
        assert_eq!(map_sqlx(sqlx::Error::PoolTimedOut).code(), "storage_error");
    }

    #[test]
    fn decode_error_maps_to_storage() {
        let parse = domain::ParseError {
            field: "status",
            value: "garbage".to_owned(),
        };
        let mapped = map_storage(queries::Error::Decode(parse));
        assert_eq!(mapped.code(), "storage_error");
    }

    #[test]
    fn validation_helper_maps_parse_error() {
        let parse = domain::ParseError {
            field: "note",
            value: String::new(),
        };
        assert!(matches!(validation(parse), Error::Validation(_)));
    }
}

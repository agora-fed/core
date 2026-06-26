//! `ProcessService` — the concrete participatory-process service. Holds the `Db` pool plus the
//! injected `Arc<dyn Clock>` and `Arc<dyn Authorization>` (ADR-0004 wiring). It drives the CRUD of
//! processes and the ordered advancement of their phases.
//!
//! Every mutation authorizes the caller via the injected [`Authorization`] (the acting citizen is
//! NEVER taken from the request body — ADR-0007), writes timestamps from the injected clock (never
//! the ambient wall clock — TESTING.md), and guards state transitions with the expected prior
//! state. This crate emits NO cross-crate events: the frozen `dsoc_core::events` catalog has no
//! `processes.*` variants, and adding one is a Tier-0 change (an ADR), so — like `dsoc-admin` — it
//! persists state and exposes routes but publishes nothing. All `sqlx` failures are mapped onto the
//! canonical [`dsoc_core::Error`] model here.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use dsoc_app::AppState;
use dsoc_core::ids::{CitizenId, OrgId};
use dsoc_core::{Authorization, Clock, Error, Result};
use dsoc_db::Db;

use crate::domain::{self, ProcessStatus};
use crate::queries::{self, PhaseRow, ProcessRow};

/// Default page size for list endpoints when the caller does not specify one.
pub const DEFAULT_PAGE_LIMIT: u32 = 50;

/// Hard cap on a list page, applied even if the caller requests more (unbounded reads must be
/// bounded — PLAN.md).
pub const MAX_PAGE_LIMIT: u32 = 100;

/// A participatory process as the service sees it (the parsed, typed view of a `process` row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Process {
    /// The process id (also its [`dsoc_core::ids::SpaceId`] handle).
    pub id: Uuid,
    /// The organization the process belongs to.
    pub org: OrgId,
    /// Stable URL-safe handle, unique within the org.
    pub slug: String,
    /// Human title (Portuguese civic content).
    pub title: String,
    /// Lifecycle state.
    pub status: ProcessStatus,
    /// Optional participation window start.
    pub starts_at: Option<DateTime<Utc>>,
    /// Optional participation window end.
    pub ends_at: Option<DateTime<Utc>>,
    /// When the process was created (injected clock).
    pub created_at: DateTime<Utc>,
}

/// The descriptive fields needed to create a process, bundled so callers pass a single value.
#[derive(Debug, Clone, Default)]
pub struct ProcessDraft {
    /// Stable URL-safe handle; validated `[a-z0-9._-]`, unique per org.
    pub slug: String,
    /// Human title; validated non-blank.
    pub title: String,
    /// Initial lifecycle state; defaults to [`ProcessStatus::Draft`] when absent.
    pub status: Option<ProcessStatus>,
    /// Optional participation window start.
    pub starts_at: Option<DateTime<Utc>>,
    /// Optional participation window end (must be after the start when both are set).
    pub ends_at: Option<DateTime<Utc>>,
}

/// A partial update to a process: each present field replaces the stored value.
#[derive(Debug, Clone, Default)]
pub struct ProcessUpdate {
    /// New title; validated non-blank when present.
    pub title: Option<String>,
    /// New lifecycle state when present.
    pub status: Option<ProcessStatus>,
}

/// An ordered stage of a process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase {
    /// Phase id.
    pub id: Uuid,
    /// The process this phase belongs to.
    pub process_id: Uuid,
    /// Human name (Portuguese civic content).
    pub name: String,
    /// Zero-based ordinal within the process.
    pub position: i32,
    /// Whether this phase is the currently open one.
    pub active: bool,
    /// When the phase was created (injected clock).
    pub created_at: DateTime<Utc>,
}

/// The participatory-process service.
#[derive(Clone)]
pub struct ProcessService {
    db: Db,
    clock: Arc<dyn Clock>,
    authz: Arc<dyn Authorization>,
}

impl std::fmt::Debug for ProcessService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessService")
            .field("db", &"PgPool")
            .finish_non_exhaustive()
    }
}

impl ProcessService {
    /// Construct from explicitly injected ports (used directly by integration tests).
    #[must_use]
    pub fn new(db: Db, clock: Arc<dyn Clock>, authz: Arc<dyn Authorization>) -> Self {
        Self { db, clock, authz }
    }

    /// Construct from the shared [`AppState`] the gateway injects (ADR-0004 wiring). The event-bus
    /// port carried by `AppState` is intentionally unused: this crate emits no cross-crate events.
    #[must_use]
    pub fn from_state(state: &AppState) -> Self {
        Self {
            db: state.db.clone(),
            clock: Arc::clone(&state.clock),
            authz: Arc::clone(&state.authz),
        }
    }

    /// Assert the caller may perform a process mutation in `org`.
    async fn authorize_mutation(&self, org: OrgId, actor: CitizenId) -> Result<()> {
        self.authz
            .require(org, actor, domain::MIN_MUTATION_LEVEL)
            .await
    }

    /// Create a participatory process.
    ///
    /// # Errors
    /// [`Error::Forbidden`] if the actor is not authorized, [`Error::Validation`] on a malformed
    /// slug/title/window, [`Error::Conflict`] if the `(org, slug)` already exists,
    /// [`Error::Storage`] otherwise.
    pub async fn create_process(
        &self,
        org: OrgId,
        actor: CitizenId,
        draft: ProcessDraft,
    ) -> Result<Process> {
        self.authorize_mutation(org, actor).await?;
        domain::validate_slug(&draft.slug)?;
        domain::validate_title(&draft.title)?;
        domain::validate_window(draft.starts_at, draft.ends_at)?;
        let status = draft.status.unwrap_or(ProcessStatus::Draft);
        let now = self.clock.now();
        let row = queries::insert_process(
            &self.db,
            Uuid::now_v7(),
            org.as_uuid(),
            &draft.slug,
            &draft.title,
            status.as_str(),
            draft.starts_at,
            draft.ends_at,
            now,
        )
        .await
        .map_err(map_sqlx)?;
        map_process(row)
    }

    /// Fetch a process. Reads are unrestricted.
    ///
    /// # Errors
    /// [`Error::NotFound`] if absent, [`Error::Storage`] on persistence failure.
    pub async fn get_process(&self, org: OrgId, id: Uuid) -> Result<Process> {
        let row = queries::get_process(&self.db, org.as_uuid(), id)
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| Error::NotFound("process not found".to_owned()))?;
        map_process(row)
    }

    /// List an organization's processes (keyset pagination over id).
    ///
    /// # Errors
    /// [`Error::Storage`] on persistence failure.
    pub async fn list_processes(
        &self,
        org: OrgId,
        after: Option<Uuid>,
        limit: Option<u32>,
    ) -> Result<Vec<Process>> {
        let rows = queries::list_processes(
            &self.db,
            org.as_uuid(),
            after,
            domain::clamp_limit(limit, MAX_PAGE_LIMIT),
        )
        .await
        .map_err(map_sqlx)?;
        rows.into_iter().map(map_process).collect()
    }

    /// Apply a partial update to a process.
    ///
    /// # Errors
    /// [`Error::Forbidden`] if unauthorized, [`Error::Validation`] on a malformed title,
    /// [`Error::NotFound`] if no such process, [`Error::Storage`] otherwise.
    pub async fn update_process(
        &self,
        org: OrgId,
        actor: CitizenId,
        id: Uuid,
        update: ProcessUpdate,
    ) -> Result<Process> {
        self.authorize_mutation(org, actor).await?;
        if let Some(title) = update.title.as_deref() {
            domain::validate_title(title)?;
        }
        let status = update.status.map(ProcessStatus::as_str);
        let row =
            queries::update_process(&self.db, org.as_uuid(), id, update.title.as_deref(), status)
                .await
                .map_err(map_sqlx)?
                .ok_or_else(|| Error::NotFound("process not found".to_owned()))?;
        map_process(row)
    }

    /// Delete a process and its phases (cascade).
    ///
    /// # Errors
    /// [`Error::Forbidden`] if unauthorized, [`Error::NotFound`] if no such process,
    /// [`Error::Storage`] otherwise.
    pub async fn delete_process(&self, org: OrgId, actor: CitizenId, id: Uuid) -> Result<()> {
        self.authorize_mutation(org, actor).await?;
        let affected = queries::delete_process(&self.db, org.as_uuid(), id)
            .await
            .map_err(map_sqlx)?;
        if affected == 0 {
            return Err(Error::NotFound("process not found".to_owned()));
        }
        Ok(())
    }

    /// Add an ordered phase to a process. New phases are created inactive; advancement opens them.
    ///
    /// # Errors
    /// [`Error::Forbidden`] if unauthorized, [`Error::Validation`] on a malformed name,
    /// [`Error::NotFound`] if the process does not exist in the org, [`Error::Conflict`] on a
    /// duplicate `position`, [`Error::Storage`] otherwise.
    pub async fn add_phase(
        &self,
        org: OrgId,
        actor: CitizenId,
        process_id: Uuid,
        name: &str,
        position: i32,
    ) -> Result<Phase> {
        self.authorize_mutation(org, actor).await?;
        domain::validate_phase_name(name)?;
        let exists = queries::process_exists(&self.db, org.as_uuid(), process_id)
            .await
            .map_err(map_sqlx)?;
        if !exists {
            return Err(Error::NotFound("process not found".to_owned()));
        }
        let now = self.clock.now();
        let row = queries::insert_phase(&self.db, Uuid::now_v7(), process_id, name, position, now)
            .await
            .map_err(map_sqlx)?;
        Ok(Phase::from(row))
    }

    /// List a process's phases (keyset pagination over position).
    ///
    /// # Errors
    /// [`Error::Storage`] on persistence failure.
    pub async fn list_phases(
        &self,
        process_id: Uuid,
        after: Option<i32>,
        limit: Option<u32>,
    ) -> Result<Vec<Phase>> {
        let rows = queries::list_phases(
            &self.db,
            process_id,
            after,
            domain::clamp_limit(limit, MAX_PAGE_LIMIT),
        )
        .await
        .map_err(map_sqlx)?;
        Ok(rows.into_iter().map(Phase::from).collect())
    }

    /// Advance a process to its next phase: deactivate the current phase and activate the next by
    /// `position` (or the first phase when none is active yet). The whole transition runs in one
    /// transaction under a `FOR UPDATE` lock on the process and its phases, guarded by the expected
    /// prior state (a closed process is read-only; the last phase has no successor).
    ///
    /// # Errors
    /// [`Error::Forbidden`] if unauthorized, [`Error::NotFound`] if no such process,
    /// [`Error::Conflict`] if the process is closed or already at its final phase,
    /// [`Error::Storage`] otherwise.
    pub async fn advance_phase(
        &self,
        org: OrgId,
        actor: CitizenId,
        process_id: Uuid,
    ) -> Result<Phase> {
        self.authorize_mutation(org, actor).await?;

        let mut tx = self.db.begin().await.map_err(map_sqlx)?;

        let lock = queries::lock_process(&mut *tx, org.as_uuid(), process_id)
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| Error::NotFound("process not found".to_owned()))?;
        let status =
            ProcessStatus::parse(&lock.status).map_err(|_| corrupt_status(&lock.status))?;
        if !status.allows_phase_advance() {
            return Err(Error::Conflict(
                "cannot advance a closed process".to_owned(),
            ));
        }

        let states = queries::lock_phase_states(&mut *tx, process_id)
            .await
            .map_err(map_sqlx)?;
        let positions: Vec<i32> = states.iter().map(|s| s.position).collect();
        let active = states.iter().find(|s| s.active).map(|s| s.position);
        let next_position = domain::next_position(&positions, active).ok_or_else(|| {
            Error::Conflict("process has no further phase to advance to".to_owned())
        })?;
        let next = states
            .iter()
            .find(|s| s.position == next_position)
            .ok_or_else(|| {
                Error::Storage(Box::new(std::io::Error::other(
                    "computed next phase missing from locked set",
                )))
            })?;

        queries::deactivate_active_phase(&mut *tx, process_id)
            .await
            .map_err(map_sqlx)?;
        let row = queries::activate_phase(&mut *tx, next.id)
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| Error::Conflict("phase was concurrently advanced".to_owned()))?;

        tx.commit().await.map_err(map_sqlx)?;
        Ok(Phase::from(row))
    }
}

/// Map a raw `sqlx` failure into the canonical, public-safe [`Error`].
fn map_sqlx(err: sqlx::Error) -> Error {
    if matches!(err, sqlx::Error::RowNotFound) {
        return Error::NotFound("process record not found".to_owned());
    }
    if let sqlx::Error::Database(ref db_err) = err {
        if db_err.is_unique_violation() {
            return Error::Conflict("process record already exists".to_owned());
        }
    }
    tracing::error!(error = %err, "processes storage failure");
    Error::Storage(Box::new(err))
}

/// A stored `status` outside the sanctioned set is a data-integrity failure (the DB `CHECK` should
/// prevent it); surface it as [`Error::Storage`], never silently degrade.
fn corrupt_status(raw: &str) -> Error {
    tracing::error!(status = %raw, "unrecognized status stored in process");
    Error::Storage(Box::new(std::io::Error::other(
        "unrecognized process status in storage",
    )))
}

/// Convert a process row, parsing its stored status (a corrupt value becomes [`Error::Storage`]).
fn map_process(row: ProcessRow) -> Result<Process> {
    let status = ProcessStatus::parse(&row.status).map_err(|_| corrupt_status(&row.status))?;
    Ok(Process {
        id: row.id,
        org: OrgId::from_uuid(row.org_id),
        slug: row.slug,
        title: row.title,
        status,
        starts_at: row.starts_at,
        ends_at: row.ends_at,
        created_at: row.created_at,
    })
}

impl From<PhaseRow> for Phase {
    fn from(row: PhaseRow) -> Self {
        Self {
            id: row.id,
            process_id: row.process_id,
            name: row.name,
            position: row.position,
            active: row.active,
            created_at: row.created_at,
        }
    }
}

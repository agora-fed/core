//! Every `sqlx` statement the crate runs, in one place and compile-time checked (PLAN.md
//! principle 3 — explicit, auditable SQL; no ORM, no `SELECT *`, keyset pagination for lists).
//! Functions return raw `sqlx::Error`; [`crate::service`] maps it onto the canonical
//! [`dsoc_core::Error`] model.

use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

/// Raw `process` row — only the columns the lifecycle needs (no `SELECT *`).
#[derive(Debug, Clone)]
pub(crate) struct ProcessRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub slug: String,
    pub title: String,
    pub status: String,
    pub starts_at: Option<DateTime<Utc>>,
    pub ends_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// Raw `process_phase` row.
#[derive(Debug, Clone)]
pub(crate) struct PhaseRow {
    pub id: Uuid,
    pub process_id: Uuid,
    pub name: String,
    pub position: i32,
    pub active: bool,
    pub created_at: DateTime<Utc>,
}

/// A minimal phase projection used to compute the next phase to activate (id + position + active).
#[derive(Debug, Clone)]
pub(crate) struct PhaseStateRow {
    pub id: Uuid,
    pub position: i32,
    pub active: bool,
}

/// The status of a process, read under a row lock during advancement (the guarded prior state).
#[derive(Debug, Clone)]
pub(crate) struct ProcessLockRow {
    pub status: String,
}

/// Insert a process. Returns the REAL stored row (RETURNING) — never a phantom id. A duplicate
/// `(org_id, slug)` raises a unique violation, mapped to [`dsoc_core::Error::Conflict`].
#[allow(clippy::too_many_arguments)]
pub(crate) async fn insert_process<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    org_id: Uuid,
    slug: &str,
    title: &str,
    status: &str,
    starts_at: Option<DateTime<Utc>>,
    ends_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
) -> Result<ProcessRow, sqlx::Error> {
    sqlx::query_as!(
        ProcessRow,
        r#"
        INSERT INTO process (id, org_id, slug, title, status, starts_at, ends_at, created_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        RETURNING id, org_id, slug, title, status, starts_at, ends_at, created_at
        "#,
        id,
        org_id,
        slug,
        title,
        status,
        starts_at,
        ends_at,
        created_at,
    )
    .fetch_one(exec)
    .await
}

/// Fetch a single process scoped to its organization. Returns `None` if absent.
pub(crate) async fn get_process<'e, E: PgExecutor<'e>>(
    exec: E,
    org_id: Uuid,
    id: Uuid,
) -> Result<Option<ProcessRow>, sqlx::Error> {
    sqlx::query_as!(
        ProcessRow,
        r#"
        SELECT id, org_id, slug, title, status, starts_at, ends_at, created_at
        FROM process
        WHERE org_id = $1 AND id = $2
        "#,
        org_id,
        id,
    )
    .fetch_optional(exec)
    .await
}

/// List an organization's processes with keyset pagination over `id` (ascending).
pub(crate) async fn list_processes<'e, E: PgExecutor<'e>>(
    exec: E,
    org_id: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<ProcessRow>, sqlx::Error> {
    sqlx::query_as!(
        ProcessRow,
        r#"
        SELECT id, org_id, slug, title, status, starts_at, ends_at, created_at
        FROM process
        WHERE org_id = $1 AND ($2::uuid IS NULL OR id > $2)
        ORDER BY id ASC
        LIMIT $3
        "#,
        org_id,
        after,
        limit,
    )
    .fetch_all(exec)
    .await
}

/// Partially update a process: `title` and/or `status` are replaced when supplied (`COALESCE`
/// keeps the existing value for a `NULL` argument). Scoped to the org; returns the updated row or
/// `None` if no such process exists (mapped to [`dsoc_core::Error::NotFound`]).
pub(crate) async fn update_process<'e, E: PgExecutor<'e>>(
    exec: E,
    org_id: Uuid,
    id: Uuid,
    title: Option<&str>,
    status: Option<&str>,
) -> Result<Option<ProcessRow>, sqlx::Error> {
    sqlx::query_as!(
        ProcessRow,
        r#"
        UPDATE process
        SET title = COALESCE($3::text, title),
            status = COALESCE($4::text, status)
        WHERE org_id = $1 AND id = $2
        RETURNING id, org_id, slug, title, status, starts_at, ends_at, created_at
        "#,
        org_id,
        id,
        title,
        status,
    )
    .fetch_optional(exec)
    .await
}

/// Delete a process (its phases cascade). Scoped to the org. Returns rows affected: 0 means no
/// such process (mapped to [`dsoc_core::Error::NotFound`]).
pub(crate) async fn delete_process<'e, E: PgExecutor<'e>>(
    exec: E,
    org_id: Uuid,
    id: Uuid,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query!(
        r#"DELETE FROM process WHERE org_id = $1 AND id = $2"#,
        org_id,
        id,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected())
}

/// Whether a process exists within an organization (the ownership guard for phase mutations).
pub(crate) async fn process_exists<'e, E: PgExecutor<'e>>(
    exec: E,
    org_id: Uuid,
    id: Uuid,
) -> Result<bool, sqlx::Error> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM process WHERE org_id = $1 AND id = $2) AS "exists!""#,
        org_id,
        id,
    )
    .fetch_one(exec)
    .await?;
    Ok(exists)
}

/// Lock a process row `FOR UPDATE` and read its status (closes the advance TOCTOU). Returns `None`
/// if the process does not exist in the org.
pub(crate) async fn lock_process<'e, E: PgExecutor<'e>>(
    exec: E,
    org_id: Uuid,
    id: Uuid,
) -> Result<Option<ProcessLockRow>, sqlx::Error> {
    sqlx::query_as!(
        ProcessLockRow,
        r#"
        SELECT status
        FROM process
        WHERE org_id = $1 AND id = $2
        FOR UPDATE
        "#,
        org_id,
        id,
    )
    .fetch_optional(exec)
    .await
}

/// Insert a phase. Returns the REAL stored row (RETURNING). A duplicate `(process_id, position)`
/// raises a unique violation, mapped to [`dsoc_core::Error::Conflict`].
pub(crate) async fn insert_phase<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    process_id: Uuid,
    name: &str,
    position: i32,
    created_at: DateTime<Utc>,
) -> Result<PhaseRow, sqlx::Error> {
    sqlx::query_as!(
        PhaseRow,
        r#"
        INSERT INTO process_phase (id, process_id, name, position, active, created_at)
        VALUES ($1, $2, $3, $4, false, $5)
        RETURNING id, process_id, name, position, active, created_at
        "#,
        id,
        process_id,
        name,
        position,
        created_at,
    )
    .fetch_one(exec)
    .await
}

/// List a process's phases with keyset pagination over `position` (ascending).
pub(crate) async fn list_phases<'e, E: PgExecutor<'e>>(
    exec: E,
    process_id: Uuid,
    after: Option<i32>,
    limit: i64,
) -> Result<Vec<PhaseRow>, sqlx::Error> {
    sqlx::query_as!(
        PhaseRow,
        r#"
        SELECT id, process_id, name, position, active, created_at
        FROM process_phase
        WHERE process_id = $1 AND ($2::int IS NULL OR position > $2)
        ORDER BY position ASC
        LIMIT $3
        "#,
        process_id,
        after,
        limit,
    )
    .fetch_all(exec)
    .await
}

/// Load and lock (`FOR UPDATE`) a process's phase states, ordered by position, to compute the next
/// phase to activate atomically with the transition.
pub(crate) async fn lock_phase_states<'e, E: PgExecutor<'e>>(
    exec: E,
    process_id: Uuid,
) -> Result<Vec<PhaseStateRow>, sqlx::Error> {
    sqlx::query_as!(
        PhaseStateRow,
        r#"
        SELECT id, position, active
        FROM process_phase
        WHERE process_id = $1
        ORDER BY position ASC
        FOR UPDATE
        "#,
        process_id,
    )
    .fetch_all(exec)
    .await
}

/// Deactivate the currently active phase of a process (guarded by `active = true`). Returns rows
/// affected (0 when no phase was active yet — the "activate the first phase" case).
pub(crate) async fn deactivate_active_phase<'e, E: PgExecutor<'e>>(
    exec: E,
    process_id: Uuid,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query!(
        r#"UPDATE process_phase SET active = false WHERE process_id = $1 AND active = true"#,
        process_id,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected())
}

/// Activate a specific phase, guarded by the expected prior state `active = false`. Returns the
/// newly activated row, or `None` if the guard did not hold (already active / absent).
pub(crate) async fn activate_phase<'e, E: PgExecutor<'e>>(
    exec: E,
    phase_id: Uuid,
) -> Result<Option<PhaseRow>, sqlx::Error> {
    sqlx::query_as!(
        PhaseRow,
        r#"
        UPDATE process_phase
        SET active = true
        WHERE id = $1 AND active = false
        RETURNING id, process_id, name, position, active, created_at
        "#,
        phase_id,
    )
    .fetch_optional(exec)
    .await
}

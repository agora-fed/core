//! Every `sqlx` statement the crate runs, in one place and compile-time checked (PLAN.md
//! principle 3 — explicit, auditable SQL; no ORM, no `SELECT *`, keyset pagination for lists).
//! Functions return raw row structs and the bare `sqlx::Error`; [`crate::service`] maps both onto
//! the domain and the canonical [`dsoc_core::Error`] model.

use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

/// Raw `initiative` row (all columns; no `SELECT *`).
#[derive(Debug, Clone)]
pub(crate) struct InitiativeRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub title: String,
    pub body: String,
    pub threshold: i32,
    pub signature_count: i64,
    pub reached_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// The escalation-relevant state returned by the atomic increment (post-update snapshot).
#[derive(Debug, Clone)]
pub(crate) struct InitiativeCountRow {
    pub signature_count: i64,
    pub threshold: i32,
    pub reached_at: Option<DateTime<Utc>>,
}

/// Persist a fresh initiative. `signature_count` starts at 0 and `reached_at` at NULL (column
/// defaults / explicit NULL). Returns the REAL stored row (RETURNING) — never a phantom id.
pub(crate) async fn insert_initiative<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    org_id: Uuid,
    title: &str,
    body: &str,
    threshold: i32,
    created_at: DateTime<Utc>,
) -> Result<InitiativeRow, sqlx::Error> {
    let row = sqlx::query_as!(
        InitiativeRow,
        r#"
        INSERT INTO initiative
            (id, org_id, title, body, threshold, signature_count, reached_at, created_at)
        VALUES ($1, $2, $3, $4, $5, 0, NULL, $6)
        RETURNING id, org_id, title, body, threshold, signature_count, reached_at, created_at
        "#,
        id,
        org_id,
        title,
        body,
        threshold,
        created_at,
    )
    .fetch_one(exec)
    .await?;
    Ok(row)
}

/// Fetch a single initiative scoped to its organization. `None` when absent.
pub(crate) async fn get_initiative<'e, E: PgExecutor<'e>>(
    exec: E,
    org_id: Uuid,
    id: Uuid,
) -> Result<Option<InitiativeRow>, sqlx::Error> {
    let row = sqlx::query_as!(
        InitiativeRow,
        r#"
        SELECT id, org_id, title, body, threshold, signature_count, reached_at, created_at
        FROM initiative
        WHERE org_id = $1 AND id = $2
        "#,
        org_id,
        id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

/// List an org's initiatives with keyset pagination over `id` (ascending). The cursor is the last
/// id of the previous page; pass `None` for the first page.
pub(crate) async fn list_initiatives<'e, E: PgExecutor<'e>>(
    exec: E,
    org_id: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<InitiativeRow>, sqlx::Error> {
    let rows = sqlx::query_as!(
        InitiativeRow,
        r#"
        SELECT id, org_id, title, body, threshold, signature_count, reached_at, created_at
        FROM initiative
        WHERE org_id = $1 AND ($2::uuid IS NULL OR id > $2)
        ORDER BY id ASC
        LIMIT $3
        "#,
        org_id,
        after,
        limit,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

/// Insert a signature idempotently: a duplicate `(initiative_id, citizen_id)` is a no-op
/// (`ON CONFLICT DO NOTHING`). Returns `Some(id)` with the REAL stored id when newly inserted,
/// `None` when the citizen had already signed (caller then re-selects via [`get_signature_id`]).
pub(crate) async fn insert_signature_if_absent<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    initiative_id: Uuid,
    citizen_id: Uuid,
    created_at: DateTime<Utc>,
) -> Result<Option<Uuid>, sqlx::Error> {
    let row = sqlx::query_scalar!(
        r#"
        INSERT INTO initiative_signature (id, initiative_id, citizen_id, created_at)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (initiative_id, citizen_id) DO NOTHING
        RETURNING id
        "#,
        id,
        initiative_id,
        citizen_id,
        created_at,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

/// Re-select the existing signature id for `(initiative_id, citizen_id)` — used on the idempotent
/// already-signed path to return the REAL stored id rather than a phantom.
pub(crate) async fn get_signature_id<'e, E: PgExecutor<'e>>(
    exec: E,
    initiative_id: Uuid,
    citizen_id: Uuid,
) -> Result<Uuid, sqlx::Error> {
    let id = sqlx::query_scalar!(
        r#"
        SELECT id
        FROM initiative_signature
        WHERE initiative_id = $1 AND citizen_id = $2
        "#,
        initiative_id,
        citizen_id,
    )
    .fetch_one(exec)
    .await?;
    Ok(id)
}

/// Atomically increment an initiative's signature tally by one and, in the SAME statement, set the
/// `reached_at` escalation marker iff the threshold is now met and it was not already set. The
/// `reached_at IS NULL` guard makes crossing the threshold one-shot/idempotent; the per-row write
/// lock Postgres takes serializes concurrent signs. Returns the post-update escalation state.
pub(crate) async fn increment_and_maybe_reach<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    now: DateTime<Utc>,
) -> Result<InitiativeCountRow, sqlx::Error> {
    let row = sqlx::query_as!(
        InitiativeCountRow,
        r#"
        UPDATE initiative
        SET signature_count = signature_count + 1,
            reached_at = CASE
                WHEN reached_at IS NULL AND signature_count + 1 >= threshold THEN $2
                ELSE reached_at
            END
        WHERE id = $1
        RETURNING signature_count, threshold, reached_at
        "#,
        id,
        now,
    )
    .fetch_one(exec)
    .await?;
    Ok(row)
}

/// Whether an organization exists (the [`dsoc_core::traits::Space::ensure_open`] check — an
/// initiatives space is hosted per organization).
pub(crate) async fn org_exists<'e, E: PgExecutor<'e>>(
    exec: E,
    org_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM org WHERE id = $1) AS "exists!""#,
        org_id,
    )
    .fetch_one(exec)
    .await?;
    Ok(exists)
}

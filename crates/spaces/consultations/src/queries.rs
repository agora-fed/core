//! Every `sqlx` statement the crate runs, in one place and compile-time checked (PLAN.md
//! principle 3 — explicit, auditable SQL; no ORM, no `SELECT *`, keyset pagination for lists).
//! Functions return raw `sqlx::Error`; [`crate::service`] maps it onto the canonical
//! [`dsoc_core::Error`] model. Every read scopes by `org_id` so a tenant can never observe
//! another tenant's consultations.

use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

/// A consultation row (`consultations_consultation`). Column list is explicit — never `SELECT *` —
/// so the shape is auditable at a glance.
#[derive(Debug, Clone)]
pub(crate) struct ConsultationRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub title: String,
    pub opens_at: DateTime<Utc>,
    pub closes_at: DateTime<Utc>,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

/// A question row (`consultations_consultation_question`).
#[derive(Debug, Clone)]
pub(crate) struct QuestionRow {
    pub id: Uuid,
    pub prompt: String,
    pub position: i32,
    pub created_at: DateTime<Utc>,
}

/// Insert a consultation and return the **real stored id** (`RETURNING`), never a phantom
/// pre-generated value. Runs inside the create transaction alongside the question inserts.
// The argument list mirrors the table columns 1:1 — a row struct would only obscure the
// compile-time-checked binding, so the explicit parameters are intentional here.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn insert_consultation<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    org_id: Uuid,
    title: &str,
    opens_at: DateTime<Utc>,
    closes_at: DateTime<Utc>,
    status: &str,
    created_at: DateTime<Utc>,
) -> Result<Uuid, sqlx::Error> {
    let stored = sqlx::query_scalar!(
        r#"
        INSERT INTO consultations_consultation
            (id, org_id, title, opens_at, closes_at, status, created_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id
        "#,
        id,
        org_id,
        title,
        opens_at,
        closes_at,
        status,
        created_at,
    )
    .fetch_one(exec)
    .await?;
    Ok(stored)
}

/// Insert one question for a consultation and return its stored id. Position uniqueness within a
/// consultation is enforced by the table's `UNIQUE (consultation_id, position)` constraint.
pub(crate) async fn insert_question<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    consultation_id: Uuid,
    prompt: &str,
    position: i32,
    created_at: DateTime<Utc>,
) -> Result<Uuid, sqlx::Error> {
    let stored = sqlx::query_scalar!(
        r#"
        INSERT INTO consultations_consultation_question
            (id, consultation_id, prompt, position, created_at)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id
        "#,
        id,
        consultation_id,
        prompt,
        position,
        created_at,
    )
    .fetch_one(exec)
    .await?;
    Ok(stored)
}

/// Transition a consultation `open -> closed`, guarded by the **expected prior state**: the
/// `WHERE status = 'open'` clause makes the update naturally idempotent and prevents a closed
/// consultation from being "re-closed". Returns the updated row when it fired, or `None` when no
/// open row matched (already closed, or not found — the caller disambiguates via a re-select).
pub(crate) async fn close_consultation<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    org_id: Uuid,
) -> Result<Option<ConsultationRow>, sqlx::Error> {
    let row = sqlx::query_as!(
        ConsultationRow,
        r#"
        UPDATE consultations_consultation
        SET status = 'closed'
        WHERE id = $1 AND org_id = $2 AND status = 'open'
        RETURNING id, org_id, title, opens_at, closes_at, status, created_at
        "#,
        id,
        org_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

/// Read a single consultation, scoped to its owning org. Bounded by the primary key.
pub(crate) async fn get_consultation<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    org_id: Uuid,
) -> Result<Option<ConsultationRow>, sqlx::Error> {
    let row = sqlx::query_as!(
        ConsultationRow,
        r#"
        SELECT id, org_id, title, opens_at, closes_at, status, created_at
        FROM consultations_consultation
        WHERE id = $1 AND org_id = $2
        "#,
        id,
        org_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

/// Read just the status of a consultation (for the [`crate::space`] open-check). Scoped by org.
pub(crate) async fn consultation_status<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    org_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let status = sqlx::query_scalar!(
        r#"
        SELECT status
        FROM consultations_consultation
        WHERE id = $1 AND org_id = $2
        "#,
        id,
        org_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(status)
}

/// List a consultation's questions in display order. Bounded by the parent consultation.
pub(crate) async fn list_questions<'e, E: PgExecutor<'e>>(
    exec: E,
    consultation_id: Uuid,
) -> Result<Vec<QuestionRow>, sqlx::Error> {
    let rows = sqlx::query_as!(
        QuestionRow,
        r#"
        SELECT id, prompt, position, created_at
        FROM consultations_consultation_question
        WHERE consultation_id = $1
        ORDER BY position ASC
        "#,
        consultation_id,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

/// Keyset-paginated browse of an org's consultations, ascending by `id` (UUIDv7 ⇒ creation order).
/// `after` is the last id of the previous page; pass `None` for the first page. Bounded by `limit`
/// (the caller clamps it) — no unbounded read.
pub(crate) async fn list_consultations<'e, E: PgExecutor<'e>>(
    exec: E,
    org_id: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<ConsultationRow>, sqlx::Error> {
    let rows = sqlx::query_as!(
        ConsultationRow,
        r#"
        SELECT id, org_id, title, opens_at, closes_at, status, created_at
        FROM consultations_consultation
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

/// Count an org's consultations (for list pagination metadata). Scoped by org.
pub(crate) async fn count_consultations<'e, E: PgExecutor<'e>>(
    exec: E,
    org_id: Uuid,
) -> Result<i64, sqlx::Error> {
    let total = sqlx::query_scalar!(
        r#"
        SELECT count(*) AS "count!"
        FROM consultations_consultation
        WHERE org_id = $1
        "#,
        org_id,
    )
    .fetch_one(exec)
    .await?;
    Ok(total)
}

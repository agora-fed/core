//! All persistence for `dsoc-notify`: compile-time-checked `sqlx` statements only.
//!
//! No ORM, no query builder, no `SELECT *` (PLAN.md principle 3). Every column is
//! listed explicitly. The `jsonb` payload is bound/read as text via an explicit cast
//! so the crate needs no extra `sqlx` JSON feature; JSON shaping stays in the domain.

use chrono::{DateTime, Utc};
use dsoc_db::Db;
use uuid::Uuid;

/// A single outbox row as needed by the dispatch logic.
#[derive(Debug, Clone)]
pub struct OutboxRow {
    /// Notification id.
    pub id: Uuid,
    /// Owning organization.
    pub org_id: Uuid,
    /// Target citizen.
    pub citizen_id: Uuid,
    /// Channel string (`push`/`email`/`whatsapp`).
    pub channel: String,
    /// JSON payload, serialized as text.
    pub payload: String,
    /// Lifecycle status string.
    pub status: String,
    /// Number of send attempts so far.
    pub attempts: i32,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

/// Insert a new pending outbox row.
///
/// # Errors
/// Propagates `sqlx::Error` (mapped to `dsoc_core::Error` by the service layer).
#[allow(clippy::too_many_arguments)]
pub async fn insert_outbox(
    db: &Db,
    id: Uuid,
    org_id: Uuid,
    citizen_id: Uuid,
    channel: &str,
    payload_json: &str,
    status: &str,
    created_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO notify_outbox
            (id, org_id, citizen_id, channel, payload, status, attempts, created_at)
        VALUES ($1, $2, $3, $4, $5::text::jsonb, $6, 0, $7)
        "#,
        id,
        org_id,
        citizen_id,
        channel,
        payload_json,
        status,
        created_at,
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Fetch a single outbox row by id.
///
/// # Errors
/// Returns `sqlx::Error::RowNotFound` if no row matches; other errors propagate.
pub async fn fetch_outbox(db: &Db, id: Uuid) -> Result<OutboxRow, sqlx::Error> {
    let row = sqlx::query!(
        r#"
        SELECT id, org_id, citizen_id, channel,
               payload::text AS "payload!", status, attempts, created_at
        FROM notify_outbox
        WHERE id = $1
        "#,
        id,
    )
    .fetch_one(db)
    .await?;
    Ok(OutboxRow {
        id: row.id,
        org_id: row.org_id,
        citizen_id: row.citizen_id,
        channel: row.channel,
        payload: row.payload,
        status: row.status,
        attempts: row.attempts,
        created_at: row.created_at,
    })
}

/// Transition an outbox row's status and attempt count under optimistic concurrency: the
/// update only applies when the row is still in the exact `(expected_status, expected_attempts)`
/// state the caller read. Returns `true` when the row was transitioned, `false` when another
/// dispatcher advanced it first (a TOCTOU race the caller must treat as a conflict).
///
/// # Errors
/// Propagates `sqlx::Error`.
pub async fn update_outbox_state_guarded(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    status: &str,
    attempts: i32,
    expected_status: &str,
    expected_attempts: i32,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        r#"
        UPDATE notify_outbox
        SET status = $2, attempts = $3
        WHERE id = $1 AND status = $4 AND attempts = $5
        "#,
        id,
        status,
        attempts,
        expected_status,
        expected_attempts,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Append a delivery receipt for a send attempt.
///
/// # Errors
/// Propagates `sqlx::Error`.
pub async fn insert_receipt(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    notification_id: Uuid,
    status: &str,
    detail: Option<&str>,
    created_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO notify_delivery_receipt
            (id, notification_id, status, detail, created_at)
        VALUES ($1, $2, $3, $4, $5)
        "#,
        id,
        notification_id,
        status,
        detail,
        created_at,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// Count receipts recorded for a notification (used by tests/diagnostics).
///
/// # Errors
/// Propagates `sqlx::Error`.
pub async fn count_receipts(db: &Db, notification_id: Uuid) -> Result<i64, sqlx::Error> {
    let row = sqlx::query!(
        r#"
        SELECT COUNT(*) AS "count!"
        FROM notify_delivery_receipt
        WHERE notification_id = $1
        "#,
        notification_id,
    )
    .fetch_one(db)
    .await?;
    Ok(row.count)
}

/// Register (idempotently) a device token for push delivery and return the id of the stored
/// row. A repeat of an existing `(citizen, platform, token)` triple is not a conflict: the
/// `ON CONFLICT ... DO UPDATE ... RETURNING id` re-selects the *existing* row's id, so the
/// caller always receives the real persisted id (never a discarded, pre-generated phantom id
/// that would later resolve to `NotFound`).
///
/// # Errors
/// Propagates `sqlx::Error`.
pub async fn insert_device_token(
    db: &Db,
    id: Uuid,
    citizen_id: Uuid,
    platform: &str,
    token: &str,
    created_at: DateTime<Utc>,
) -> Result<Uuid, sqlx::Error> {
    let row = sqlx::query!(
        r#"
        INSERT INTO notify_device_token
            (id, citizen_id, platform, token, created_at)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (citizen_id, platform, token)
            DO UPDATE SET created_at = EXCLUDED.created_at
        RETURNING id
        "#,
        id,
        citizen_id,
        platform,
        token,
        created_at,
    )
    .fetch_one(db)
    .await?;
    Ok(row.id)
}

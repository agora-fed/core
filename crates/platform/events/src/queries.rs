//! Every `sqlx` statement for the durable event bus lives here, compile-time-checked against
//! the real schema. No ORM, no query builder, no `SELECT *`; reads are keyset-paginated.
//!
//! `jsonb` is read/written as text (`payload::text` / `$n::text::jsonb`) so the crate needs no
//! extra `sqlx` JSON feature: the `Event` JSON is owned by [`crate::domain`], not by `sqlx`.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use dsoc_db::Db;

/// The columns of `events_log` needed to rebuild an `EventEnvelope` (payload projected to text).
/// `topic`/`event_type`/`processed_at` drive the `WHERE` clause but are not re-read into Rust.
#[derive(Debug)]
pub(crate) struct EventLogRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub payload: String,
    pub occurred_at: DateTime<Utc>,
}

/// Append one delivery to the durable log. The caller supplies `id`/`occurred_at` (from the
/// injected clock upstream); `processed_at` defaults to NULL (unhandled).
pub(crate) async fn insert_event(
    db: &Db,
    id: Uuid,
    org_id: Uuid,
    topic: &str,
    event_type: &str,
    payload: &str,
    occurred_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"INSERT INTO events_log (id, org_id, topic, event_type, payload, occurred_at)
           VALUES ($1, $2, $3, $4, $5::text::jsonb, $6)"#,
        id,
        org_id,
        topic,
        event_type,
        payload,
        occurred_at,
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Fetch unprocessed deliveries on `topic`, keyset-paginated strictly after `(after_occurred_at,
/// after_id)`. A NULL cursor starts from the beginning. Ordered by `(occurred_at, id)`.
pub(crate) async fn fetch_unprocessed(
    db: &Db,
    topic: &str,
    after_occurred_at: Option<DateTime<Utc>>,
    after_id: Option<Uuid>,
    limit: i64,
) -> Result<Vec<EventLogRow>, sqlx::Error> {
    sqlx::query_as!(
        EventLogRow,
        r#"SELECT id,
                  org_id,
                  payload::text AS "payload!",
                  occurred_at
           FROM events_log
           WHERE processed_at IS NULL
             AND topic = $1
             AND ($2::timestamptz IS NULL OR (occurred_at, id) > ($2, $3))
           ORDER BY occurred_at, id
           LIMIT $4"#,
        topic,
        after_occurred_at,
        after_id,
        limit,
    )
    .fetch_all(db)
    .await
}

/// Mark a delivery handled. Idempotent: a repeat keeps the first `processed_at`. Returns the id
/// if the row exists, `None` if it does not (so the service can raise `NotFound`).
pub(crate) async fn mark_processed(
    db: &Db,
    id: Uuid,
    processed_at: DateTime<Utc>,
) -> Result<Option<Uuid>, sqlx::Error> {
    let rec = sqlx::query!(
        r#"UPDATE events_log
           SET processed_at = COALESCE(processed_at, $2)
           WHERE id = $1
           RETURNING id"#,
        id,
        processed_at,
    )
    .fetch_optional(db)
    .await?;
    Ok(rec.map(|r| r.id))
}

/// Count unprocessed deliveries grouped by topic (queue depth for the admin/health endpoint).
pub(crate) async fn queue_depth(db: &Db) -> Result<Vec<(String, i64)>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT topic AS "topic!", COUNT(*) AS "pending!"
           FROM events_log
           WHERE processed_at IS NULL
           GROUP BY topic
           ORDER BY topic"#
    )
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(|r| (r.topic, r.pending)).collect())
}

/// Read a subscriber's keyset cursor for `topic`, if one has been recorded.
pub(crate) async fn get_cursor(
    db: &Db,
    subscriber: &str,
    topic: &str,
) -> Result<Option<(DateTime<Utc>, Uuid)>, sqlx::Error> {
    let rec = sqlx::query!(
        r#"SELECT cursor_occurred_at, cursor_event_id
           FROM events_subscription
           WHERE subscriber = $1 AND topic = $2"#,
        subscriber,
        topic,
    )
    .fetch_optional(db)
    .await?;
    Ok(
        rec.and_then(|r| match (r.cursor_occurred_at, r.cursor_event_id) {
            (Some(at), Some(id)) => Some((at, id)),
            _ => None,
        }),
    )
}

/// Advance (or create) a subscriber's keyset cursor for `topic`.
pub(crate) async fn upsert_cursor(
    db: &Db,
    subscriber: &str,
    topic: &str,
    cursor_occurred_at: DateTime<Utc>,
    cursor_event_id: Uuid,
    updated_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"INSERT INTO events_subscription
               (subscriber, topic, cursor_occurred_at, cursor_event_id, updated_at)
           VALUES ($1, $2, $3, $4, $5)
           ON CONFLICT (subscriber, topic) DO UPDATE
               SET cursor_occurred_at = EXCLUDED.cursor_occurred_at,
                   cursor_event_id    = EXCLUDED.cursor_event_id,
                   updated_at         = EXCLUDED.updated_at"#,
        subscriber,
        topic,
        cursor_occurred_at,
        cursor_event_id,
        updated_at,
    )
    .execute(db)
    .await?;
    Ok(())
}

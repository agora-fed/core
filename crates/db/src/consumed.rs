//! Consumer idempotency (ADR-0007). At-least-once delivery means a consumer may see the same event
//! twice; `claim_consumed` records `(consumer, event_id)` and returns whether THIS delivery is the
//! first, so handlers apply their effect exactly once. State-based guards (e.g. `WHERE
//! threshold_crossed_at IS NULL`) remain acceptable where they are naturally idempotent; this is the
//! general mechanism for the rest.

use dsoc_core::ids::EventId;

/// Atomically claim an event for a consumer. Returns `true` if newly claimed (process it), `false`
/// if already processed (skip). Call inside the same transaction as the effect.
///
/// # Errors
/// Returns the underlying `sqlx::Error` on failure.
pub async fn claim_consumed<'e, E>(
    executor: E,
    consumer: &str,
    event_id: EventId,
) -> Result<bool, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    let inserted = sqlx::query_scalar::<_, bool>(
        "INSERT INTO events_consumed (consumer, event_id, consumed_at) \
         VALUES ($1, $2, now()) ON CONFLICT (consumer, event_id) DO NOTHING RETURNING true",
    )
    .bind(consumer)
    .bind(event_id.as_uuid())
    .fetch_optional(executor)
    .await?;
    Ok(inserted.unwrap_or(false))
}

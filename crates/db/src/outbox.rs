//! Transactional outbox — the canonical way to emit a cross-crate event from a domain mutation
//! (ADR-0006). The event row is written to `events_log` **inside the caller's transaction**, so the
//! domain change and the emission commit atomically: there is no dual-write window where a committed
//! domain change can lose its event (or a published event can reference an uncommitted change).
//!
//! The `dsoc-events` dispatcher delivers from `events_log` afterwards (at-least-once; consumers must
//! be idempotent). Use this for any state change that emits an event. The `dsoc_core::EventBus` port
//! remains for fire-and-forget emission outside a domain transaction.

use dsoc_core::events::EventEnvelope;

/// Append an event to the outbox (`events_log`) using the caller's executor — pass `&mut *tx` to make
/// it part of an open transaction.
///
/// # Errors
/// Returns the underlying `sqlx::Error` if the insert fails.
pub async fn publish_tx<'e, E>(executor: E, envelope: &EventEnvelope) -> Result<(), sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    // The full tagged Event JSON is the payload (round-trips back to Event); the serde `type` tag is
    // stored separately for cheap filtering. Serialization of our own types is infallible in practice.
    let value =
        serde_json::to_value(&envelope.event).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
    let event_type = value
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| envelope.event.topic().as_str());
    let payload = value.to_string();

    sqlx::query(
        "INSERT INTO events_log (id, org_id, topic, event_type, payload, occurred_at) \
         VALUES ($1, $2, $3, $4, $5::text::jsonb, $6)",
    )
    .bind(envelope.id.as_uuid())
    .bind(envelope.org.as_uuid())
    .bind(envelope.event.topic().as_str())
    .bind(event_type)
    .bind(payload)
    .bind(envelope.at)
    .execute(executor)
    .await?;
    Ok(())
}

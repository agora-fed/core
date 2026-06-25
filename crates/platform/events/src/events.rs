//! The consume side of the bus. `dsoc-events` emits no events of its own (its contract is
//! `consumes: *`); it provides the durable dispatch loop that drives a subscriber's handler over
//! unprocessed deliveries and advances its cursor. Handlers must be idempotent (at-least-once).

use dsoc_core::events::{EventEnvelope, EventTopic};
use dsoc_core::Result;

use crate::domain::{Cursor, SubscriberName};
use crate::service::EventQueue;

/// A consumer of durable deliveries. Implementations must be idempotent: a delivery may be
/// presented more than once (at-least-once semantics; the cursor only advances after handling).
#[async_trait::async_trait]
pub trait EventHandler: Send + Sync {
    /// Handle one delivery. Returning `Err` aborts the batch before the cursor advances, so the
    /// delivery is retried on the next poll.
    async fn handle(&self, envelope: &EventEnvelope) -> Result<()>;
}

/// Drain one batch of unprocessed deliveries on `topic` for `subscriber`: for each delivery, run
/// `handler`, mark it processed, and (on success of the whole batch) advance the durable cursor.
/// Returns the number of deliveries handled.
///
/// Processing stops at the first handler error, leaving the cursor at the last fully handled
/// delivery so the batch resumes from there.
///
/// # Errors
/// Propagates any handler or storage [`Error`](dsoc_core::Error). Partial progress is durable:
/// already-handled deliveries stay marked processed.
pub async fn dispatch_batch<H: EventHandler + ?Sized>(
    queue: &EventQueue,
    subscriber: &SubscriberName,
    topic: EventTopic,
    limit: u32,
    handler: &H,
) -> Result<usize> {
    let batch = queue.poll_for_subscriber(subscriber, topic, limit).await?;
    let mut handled = 0usize;
    let mut last_cursor: Option<Cursor> = None;

    for envelope in &batch {
        handler.handle(envelope).await?;
        queue.mark_processed(envelope.id).await?;
        last_cursor = Some(Cursor {
            occurred_at: envelope.at,
            id: envelope.id.as_uuid(),
        });
        handled += 1;
    }

    if let Some(cursor) = last_cursor {
        queue.advance_cursor(subscriber, topic, cursor).await?;
    }

    Ok(handled)
}

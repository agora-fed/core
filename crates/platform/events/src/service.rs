//! The service layer: the concrete [`PgEventBus`] (the publish port) and [`EventQueue`] (poll,
//! mark-processed, queue depth, and durable per-subscriber cursors). All `sqlx` failures are
//! mapped onto the canonical [`dsoc_core::Error`] so the gateway renders non-leaking responses.

use std::sync::Arc;

use dsoc_core::clock::Clock;
use dsoc_core::events::{Event, EventEnvelope, EventTopic};
use dsoc_core::ids::{EventId, OrgId};
use dsoc_core::traits::EventBus;
use dsoc_core::{Error, Result};
use dsoc_db::Db;

use crate::domain::{self, Cursor, QueueDepth, SubscriberName};
use crate::queries::{self, EventLogRow};

/// Largest page a single poll may return, bounding memory and query cost.
const MAX_POLL_LIMIT: u32 = 500;

/// Translate a `sqlx` failure into the canonical domain [`Error`].
fn map_sqlx(err: sqlx::Error) -> Error {
    match &err {
        sqlx::Error::RowNotFound => Error::NotFound("event".into()),
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            Error::Conflict("event already recorded".into())
        }
        _ => Error::Storage(Box::new(err)),
    }
}

/// Rebuild a durable [`EventEnvelope`] from a stored row.
fn row_to_envelope(row: EventLogRow) -> Result<EventEnvelope> {
    let event: Event = domain::decode_event(&row.payload)?;
    Ok(EventEnvelope {
        id: EventId::from_uuid(row.id),
        org: OrgId::from_uuid(row.org_id),
        at: row.occurred_at,
        event,
    })
}

/// The concrete Postgres-backed publish port. Implements [`EventBus`] by appending an
/// [`EventEnvelope`] row to `events_log`. Injected workspace-wide as `Arc<dyn EventBus>`.
#[derive(Debug, Clone)]
pub struct PgEventBus {
    db: Db,
}

impl PgEventBus {
    /// Build a publisher over the shared pool.
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait::async_trait]
impl EventBus for PgEventBus {
    async fn publish(&self, envelope: EventEnvelope) -> Result<()> {
        let encoded = domain::encode_event(&envelope.event)?;
        queries::insert_event(
            &self.db,
            envelope.id.as_uuid(),
            envelope.org.as_uuid(),
            encoded.topic,
            &encoded.event_type,
            &encoded.payload,
            envelope.at,
        )
        .await
        .map_err(map_sqlx)
    }
}

/// The consume/dispatch side: poll unprocessed deliveries by topic (keyset), mark them handled,
/// report queue depth, and persist per-subscriber cursors. Time comes from the injected clock.
#[derive(Clone)]
pub struct EventQueue {
    db: Db,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for EventQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventQueue")
            .field("db", &"PgPool")
            .finish_non_exhaustive()
    }
}

impl EventQueue {
    /// Build the queue over the shared pool and injected clock.
    #[must_use]
    pub fn new(db: Db, clock: Arc<dyn Clock>) -> Self {
        Self { db, clock }
    }

    /// Poll unprocessed deliveries on `topic`, strictly after `after`, up to `limit`
    /// (clamped to `[1, MAX_POLL_LIMIT]`). Results are ordered by `(occurred_at, id)`.
    ///
    /// # Errors
    /// [`Error::Storage`] on a query failure, or if a stored payload fails to decode.
    pub async fn poll(
        &self,
        topic: EventTopic,
        after: Option<Cursor>,
        limit: u32,
    ) -> Result<Vec<EventEnvelope>> {
        let effective = limit.clamp(1, MAX_POLL_LIMIT);
        let rows = queries::fetch_unprocessed(
            &self.db,
            topic.as_str(),
            after.map(|c| c.occurred_at),
            after.map(|c| c.id),
            i64::from(effective),
        )
        .await
        .map_err(map_sqlx)?;
        rows.into_iter().map(row_to_envelope).collect()
    }

    /// Poll on behalf of `subscriber`, resuming from its durably stored cursor.
    ///
    /// # Errors
    /// [`Error::Storage`] on a query failure, or if a stored payload fails to decode.
    pub async fn poll_for_subscriber(
        &self,
        subscriber: &SubscriberName,
        topic: EventTopic,
        limit: u32,
    ) -> Result<Vec<EventEnvelope>> {
        let cursor = queries::get_cursor(&self.db, subscriber.as_str(), topic.as_str())
            .await
            .map_err(map_sqlx)?
            .map(|(occurred_at, id)| Cursor { occurred_at, id });
        self.poll(topic, cursor, limit).await
    }

    /// Mark a delivery handled (idempotent). Errors with [`Error::NotFound`] if the id is unknown.
    ///
    /// # Errors
    /// [`Error::NotFound`] if no such delivery exists; [`Error::Storage`] on a query failure.
    pub async fn mark_processed(&self, id: EventId) -> Result<()> {
        let now = self.clock.now();
        match queries::mark_processed(&self.db, id.as_uuid(), now)
            .await
            .map_err(map_sqlx)?
        {
            Some(_) => Ok(()),
            None => Err(Error::NotFound(format!("event {id}"))),
        }
    }

    /// Persist `subscriber`'s keyset cursor for `topic`.
    ///
    /// # Errors
    /// [`Error::Storage`] on a query failure.
    pub async fn advance_cursor(
        &self,
        subscriber: &SubscriberName,
        topic: EventTopic,
        cursor: Cursor,
    ) -> Result<()> {
        let now = self.clock.now();
        queries::upsert_cursor(
            &self.db,
            subscriber.as_str(),
            topic.as_str(),
            cursor.occurred_at,
            cursor.id,
            now,
        )
        .await
        .map_err(map_sqlx)
    }

    /// Count unprocessed deliveries per topic (queue depth for the admin/health endpoint).
    ///
    /// # Errors
    /// [`Error::Storage`] on a query failure.
    pub async fn queue_depth(&self) -> Result<Vec<QueueDepth>> {
        let rows = queries::queue_depth(&self.db).await.map_err(map_sqlx)?;
        Ok(rows
            .into_iter()
            .map(|(topic, pending)| QueueDepth { topic, pending })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_not_found_maps_to_not_found() {
        assert_eq!(map_sqlx(sqlx::Error::RowNotFound).code(), "not_found");
    }

    #[test]
    fn other_sqlx_errors_map_to_storage() {
        let err = map_sqlx(sqlx::Error::PoolClosed);
        assert_eq!(err.code(), "storage_error");
    }
}

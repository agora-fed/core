//! The notification service: enqueue → dispatch → receipt, with bounded retries.
//!
//! Holds the Tier-0 ports ([`Db`], `Arc<dyn Clock>`, `Arc<dyn EventBus>`) and maps every
//! failure onto the canonical [`dsoc_core::Error`]. Channels are abstracted behind the
//! [`Channel`] trait so real SMTP/WhatsApp transports are never touched in tests.

use std::sync::Arc;

use async_trait::async_trait;
use dsoc_core::clock::Clock;
use dsoc_core::error::{Error, Result};
use dsoc_core::events::EventEnvelope;
use dsoc_core::ids::{CitizenId, NotificationId, OrgId};
use dsoc_core::traits::{Authorization, VerificationLevel};
use dsoc_db::Db;
use uuid::Uuid;

use crate::domain::{
    apply_attempt, plan_for_event, ChannelKind, DeviceTokenInput, NotificationPlan, OutboxStatus,
};
use crate::events::signal_envelope;
use crate::queries;

/// Minimum verification a caller must hold to register a push device token: an
/// email-confirmed citizen acting for themselves (notification-hijacking defence).
const MIN_DEVICE_TOKEN_LEVEL: VerificationLevel = VerificationLevel::Email;

/// A message handed to a transport for delivery. Channel-agnostic; the transport
/// renders it for its medium.
#[derive(Debug, Clone)]
pub struct OutboundMessage {
    /// The notification this message belongs to.
    pub notification: NotificationId,
    /// Target citizen.
    pub citizen: CitizenId,
    /// Channel the message is being sent on.
    pub channel: ChannelKind,
    /// Channel-agnostic JSON body.
    pub payload: serde_json::Value,
}

/// A transport-level send failure (never carries secrets; safe to log).
#[derive(Debug, Clone)]
pub struct ChannelError(pub String);

impl std::fmt::Display for ChannelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "channel send failed: {}", self.0)
    }
}

impl std::error::Error for ChannelError {}

/// A delivery channel. Production crates wire real SMTP/push/WhatsApp transports;
/// tests use [`NoopChannel`] or a failing double. No transport is called from this crate.
#[async_trait]
pub trait Channel: Send + Sync {
    /// Attempt to deliver `message`. `Ok(())` means the transport accepted it.
    ///
    /// # Errors
    /// Returns [`ChannelError`] if the transport rejected the message.
    async fn send(&self, message: &OutboundMessage) -> std::result::Result<(), ChannelError>;
}

/// A no-op channel that always accepts. The default safe transport for tests and for
/// environments where no real provider is configured (never calls the network).
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopChannel;

#[async_trait]
impl Channel for NoopChannel {
    async fn send(&self, _message: &OutboundMessage) -> std::result::Result<(), ChannelError> {
        Ok(())
    }
}

/// Outcome of a single dispatch attempt, surfaced for diagnostics and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryReport {
    /// Resulting outbox status after the attempt.
    pub status: OutboxStatus,
    /// Attempt count after this attempt.
    pub attempts: i32,
    /// Whether the transport accepted the message on this attempt.
    pub accepted: bool,
}

/// The notification service.
#[derive(Clone)]
pub struct NotifyService {
    db: Db,
    clock: Arc<dyn Clock>,
    authz: Arc<dyn Authorization>,
}

impl std::fmt::Debug for NotifyService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotifyService")
            .field("db", &"PgPool")
            .finish_non_exhaustive()
    }
}

impl NotifyService {
    /// Construct a service from the injected Tier-0 ports.
    #[must_use]
    pub fn new(db: Db, clock: Arc<dyn Clock>, authz: Arc<dyn Authorization>) -> Self {
        Self { db, clock, authz }
    }

    /// Build a service from the shared [`dsoc_app::AppState`].
    #[must_use]
    pub fn from_state(state: &dsoc_app::AppState) -> Self {
        Self::new(
            state.db.clone(),
            state.clock.clone(),
            Arc::clone(&state.authz),
        )
    }

    /// Register (idempotently) a push device token for a citizen, returning the id of the
    /// stored row.
    ///
    /// The caller must be an email-verified citizen of `org`; the HTTP layer additionally
    /// asserts the caller is registering a token for *themselves*. Re-registering the same
    /// `(citizen, platform, token)` triple is idempotent and returns the same persisted id.
    ///
    /// # Errors
    /// [`Error::Forbidden`] if the citizen is not sufficiently verified; [`Error::Validation`]
    /// for a bad platform/token; [`Error::Storage`] on a DB failure.
    pub async fn register_device_token(
        &self,
        org: OrgId,
        citizen: CitizenId,
        platform: &str,
        token: &str,
    ) -> Result<Uuid> {
        self.authz
            .require(org, citizen, MIN_DEVICE_TOKEN_LEVEL)
            .await?;
        let input = DeviceTokenInput::parse(platform, token)?;
        let id = queries::insert_device_token(
            &self.db,
            Uuid::now_v7(),
            citizen.as_uuid(),
            input.platform.as_str(),
            &input.token,
            self.clock.now(),
        )
        .await
        .map_err(map_sqlx)?;
        Ok(id)
    }

    /// Enqueue a notification on a channel for later dispatch.
    ///
    /// # Errors
    /// [`Error::Conflict`] on a unique violation; [`Error::Storage`] otherwise.
    pub async fn enqueue(
        &self,
        org: OrgId,
        citizen: CitizenId,
        channel: ChannelKind,
        payload: &serde_json::Value,
    ) -> Result<NotificationId> {
        let id = NotificationId::new();
        let payload_json =
            serde_json::to_string(payload).map_err(|e| Error::Storage(Box::new(e)))?;
        queries::insert_outbox(
            &self.db,
            id.as_uuid(),
            org.as_uuid(),
            citizen.as_uuid(),
            channel.as_str(),
            &payload_json,
            OutboxStatus::Pending.as_str(),
            self.clock.now(),
        )
        .await
        .map_err(map_sqlx)?;
        Ok(id)
    }

    /// Consume an inbound event envelope and, if it maps to a notification, enqueue an
    /// outbox row for `recipient`. Returns `None` for events this crate ignores, so the
    /// consumer is idempotent and total over the event catalog.
    ///
    /// # Errors
    /// Propagates storage errors from [`Self::enqueue`].
    pub async fn enqueue_for_event(
        &self,
        envelope: &EventEnvelope,
        recipient: CitizenId,
    ) -> Result<Option<NotificationId>> {
        match plan_for_event(&envelope.event) {
            Some(NotificationPlan { channel, payload }) => {
                let id = self
                    .enqueue(envelope.org, recipient, channel, &payload)
                    .await?;
                Ok(Some(id))
            }
            None => Ok(None),
        }
    }

    /// Attempt to dispatch a pending notification through `channel`, write a receipt, and
    /// emit the terminal event when the lifecycle reaches `dispatched` or `failed`.
    ///
    /// Retries are bounded by [`crate::domain::MAX_DELIVERY_ATTEMPTS`]: a transient failure
    /// leaves the row `pending` for another call; once terminal, further dispatch is rejected.
    ///
    /// # Errors
    /// [`Error::NotFound`] if the id is unknown; [`Error::Conflict`] if already terminal or if a
    /// concurrent dispatcher advanced the row first; [`Error::Storage`] on a DB failure.
    pub async fn dispatch(
        &self,
        channel: &dyn Channel,
        id: NotificationId,
    ) -> Result<DeliveryReport> {
        let row = queries::fetch_outbox(&self.db, id.as_uuid())
            .await
            .map_err(map_sqlx)?;

        let status = OutboxStatus::parse(&row.status)?;
        if status.is_terminal() {
            return Err(Error::Conflict(format!(
                "notification already {}",
                status.as_str()
            )));
        }

        let channel_kind = ChannelKind::parse(&row.channel)?;
        let payload: serde_json::Value =
            serde_json::from_str(&row.payload).map_err(|e| Error::Storage(Box::new(e)))?;
        let message = OutboundMessage {
            notification: id,
            citizen: CitizenId::from_uuid(row.citizen_id),
            channel: channel_kind,
            payload,
        };

        // The transport call is a network side effect and MUST stay outside the DB transaction
        // so a slow/wedged provider never pins a connection or holds row locks.
        let (accepted, detail) = match channel.send(&message).await {
            Ok(()) => (true, None),
            Err(ChannelError(reason)) => (false, Some(reason)),
        };

        let transition = apply_attempt(row.attempts, accepted);
        let now = self.clock.now();
        let org = OrgId::from_uuid(row.org_id);

        // Persist the state change, the receipt, and (for a terminal outcome) the cross-crate
        // event atomically. The event is written to the outbox INSIDE this transaction
        // (ADR-0006) so a delivered/failed signal can never be lost to a post-commit dual-write.
        let mut tx = self.db.begin().await.map_err(map_sqlx)?;

        // Optimistic concurrency: only transition if the row is still exactly as we read it.
        // A losing racer here did its own send but must not double-write state or events.
        let advanced = queries::update_outbox_state_guarded(
            &mut *tx,
            id.as_uuid(),
            transition.status.as_str(),
            transition.attempts,
            OutboxStatus::Pending.as_str(),
            row.attempts,
        )
        .await
        .map_err(map_sqlx)?;
        if !advanced {
            tx.rollback().await.map_err(map_sqlx)?;
            return Err(Error::Conflict(
                "notification was concurrently updated".into(),
            ));
        }

        queries::insert_receipt(
            &mut *tx,
            Uuid::now_v7(),
            id.as_uuid(),
            transition.receipt.as_str(),
            detail.as_deref(),
            now,
        )
        .await
        .map_err(map_sqlx)?;

        if let Some(signal) = transition.emit {
            let envelope = signal_envelope(org, id, now, signal);
            dsoc_db::outbox::publish_tx(&mut *tx, &envelope)
                .await
                .map_err(map_sqlx)?;
        }

        tx.commit().await.map_err(map_sqlx)?;

        Ok(DeliveryReport {
            status: transition.status,
            attempts: transition.attempts,
            accepted,
        })
    }
}

/// Map a `sqlx::Error` onto the canonical domain error (no internal detail leaks).
fn map_sqlx(err: sqlx::Error) -> Error {
    match &err {
        sqlx::Error::RowNotFound => Error::NotFound("notification not found".into()),
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            Error::Conflict("duplicate notification record".into())
        }
        _ => Error::Storage(Box::new(err)),
    }
}

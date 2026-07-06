//! Pure domain types and logic for multi-channel fan-out.
//!
//! This module contains **no** `sqlx` or `axum` — only value objects, validation,
//! the retry state machine, and the event→notification planning rules. Keeping it
//! pure makes the politically-sensitive delivery logic exhaustively unit-testable
//! without a database or network.

use dsoc_core::error::{Error, Result};
use dsoc_core::events::Event;
use serde_json::json;

/// Maximum number of send attempts before a notification is permanently failed.
/// Bounds retry/backoff so a wedged channel can never loop forever.
pub const MAX_DELIVERY_ATTEMPTS: i32 = 5;

/// A delivery channel for a notification (the fan-out targets).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelKind {
    /// Mobile push notification.
    Push,
    /// Transactional email (SMTP).
    Email,
    /// WhatsApp / Chatwoot message.
    WhatsApp,
}

impl ChannelKind {
    /// Stable database/wire string for this channel.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            ChannelKind::Push => "push",
            ChannelKind::Email => "email",
            ChannelKind::WhatsApp => "whatsapp",
        }
    }

    /// Parse a channel from its stable string, validating at the boundary.
    ///
    /// # Errors
    /// Returns [`Error::Validation`] if `s` is not a known channel.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "push" => Ok(ChannelKind::Push),
            "email" => Ok(ChannelKind::Email),
            "whatsapp" => Ok(ChannelKind::WhatsApp),
            other => Err(Error::Validation(format!("unknown channel '{other}'"))),
        }
    }
}

/// Lifecycle state of an outbox row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboxStatus {
    /// Awaiting dispatch (or awaiting a bounded retry after a transient failure).
    Pending,
    /// Successfully handed to the channel.
    Dispatched,
    /// Permanently failed after exhausting retries.
    Failed,
}

impl OutboxStatus {
    /// Stable database/wire string for this status.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            OutboxStatus::Pending => "pending",
            OutboxStatus::Dispatched => "dispatched",
            OutboxStatus::Failed => "failed",
        }
    }

    /// Whether this is a terminal state (no further dispatch permitted).
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, OutboxStatus::Dispatched | OutboxStatus::Failed)
    }

    /// Parse a status from its stable string.
    ///
    /// # Errors
    /// Returns [`Error::Validation`] if `s` is not a known status.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "pending" => Ok(OutboxStatus::Pending),
            "dispatched" => Ok(OutboxStatus::Dispatched),
            "failed" => Ok(OutboxStatus::Failed),
            other => Err(Error::Validation(format!("unknown status '{other}'"))),
        }
    }
}

/// Outcome recorded for a single send attempt (delivery receipt status).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptStatus {
    /// The channel accepted the message.
    Delivered,
    /// The channel rejected the message.
    Failed,
}

impl ReceiptStatus {
    /// Stable database/wire string for this receipt status.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            ReceiptStatus::Delivered => "delivered",
            ReceiptStatus::Failed => "failed",
        }
    }
}

/// Push platform a device token belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevicePlatform {
    /// Apple iOS.
    Ios,
    /// Google Android.
    Android,
    /// Web push.
    Web,
}

impl DevicePlatform {
    /// Stable database/wire string for this platform.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            DevicePlatform::Ios => "ios",
            DevicePlatform::Android => "android",
            DevicePlatform::Web => "web",
        }
    }

    /// Parse a platform from its stable string.
    ///
    /// # Errors
    /// Returns [`Error::Validation`] if `s` is not a known platform.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "ios" => Ok(DevicePlatform::Ios),
            "android" => Ok(DevicePlatform::Android),
            "web" => Ok(DevicePlatform::Web),
            other => Err(Error::Validation(format!("unknown platform '{other}'"))),
        }
    }
}

/// Maximum accepted device-token length (defends storage against abuse).
pub const MAX_DEVICE_TOKEN_LEN: usize = 4096;

/// A validated device-token registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceTokenInput {
    /// Target push platform.
    pub platform: DevicePlatform,
    /// Provider-issued opaque token.
    pub token: String,
}

impl DeviceTokenInput {
    /// Validate a raw registration at the system boundary.
    ///
    /// # Errors
    /// Returns [`Error::Validation`] if the platform is unknown or the token is empty
    /// or longer than [`MAX_DEVICE_TOKEN_LEN`].
    pub fn parse(platform: &str, token: &str) -> Result<Self> {
        let platform = DevicePlatform::parse(platform)?;
        let trimmed = token.trim();
        if trimmed.is_empty() {
            return Err(Error::Validation("device token must not be empty".into()));
        }
        if trimmed.len() > MAX_DEVICE_TOKEN_LEN {
            return Err(Error::Validation("device token too long".into()));
        }
        Ok(Self {
            platform,
            token: trimmed.to_owned(),
        })
    }
}

/// A planned notification derived from a consumed event: which channel and what body.
#[derive(Debug, Clone, PartialEq)]
pub struct NotificationPlan {
    /// Channel to fan out on.
    pub channel: ChannelKind,
    /// Channel-agnostic JSON body (serialized to the `jsonb` payload column).
    pub payload: serde_json::Value,
}

/// Map a consumed cross-crate [`Event`] to a notification plan.
///
/// Returns `None` for events this crate does not act on, so the caller can ignore
/// them idempotently. Only the three events named in the crate contract are handled.
#[must_use]
pub fn plan_for_event(event: &Event) -> Option<NotificationPlan> {
    match event {
        Event::ConsequenceSlaStarted {
            sla,
            mandate,
            cluster,
            due,
        } => Some(NotificationPlan {
            // An official must be reached reliably when their SLA clock starts.
            channel: ChannelKind::Email,
            payload: json!({
                "kind": "consequence.sla.started",
                "sla": sla.as_uuid(),
                "mandate": mandate.as_uuid(),
                "cluster": cluster.as_uuid(),
                "due": due,
                "title": "SLA iniciado",
                "body": "Um SLA de resposta acaba de começar para uma demanda dirigida ao seu mandato. Responda pelo painel antes do prazo.",
            }),
        }),
        Event::ConsequenceSlaExpired { sla, mandate } => Some(NotificationPlan {
            channel: ChannelKind::Push,
            payload: json!({
                "kind": "consequence.sla.expired",
                "sla": sla.as_uuid(),
                "mandate": mandate.as_uuid(),
                "title": "SLA expirou",
                "body": "O prazo de resposta expirou. O silêncio ficará registrado no seu placar público.",
            }),
        }),
        // A directed proposal crossed its threshold — forewarn the operator before the SLA clock
        // starts. Push keeps the loop feeling live without piling on transactional email volume.
        Event::ProposalThresholdCrossed {
            proposal,
            cluster,
            mandate,
        } => Some(NotificationPlan {
            channel: ChannelKind::Push,
            payload: json!({
                "kind": "proposals.threshold.crossed",
                "proposal": proposal.as_uuid(),
                "cluster": cluster.as_uuid(),
                "mandate": mandate.as_uuid(),
                "title": "Uma demanda atingiu o limiar",
                "body": "Uma proposta dirigida ao seu mandato acaba de atingir o limiar de apoio. Um SLA de resposta vai começar em breve.",
            }),
        }),
        Event::ProposalPublished { proposal, mandate } => Some(NotificationPlan {
            channel: ChannelKind::Push,
            payload: json!({
                "kind": "proposals.published",
                "proposal": proposal.as_uuid(),
                "mandate": mandate.as_uuid(),
            }),
        }),
        _ => None,
    }
}

/// The result of applying a single dispatch attempt to an outbox row: the pure
/// state transition the service then persists and emits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchTransition {
    /// New attempt count to persist.
    pub attempts: i32,
    /// New outbox status to persist.
    pub status: OutboxStatus,
    /// Receipt to append for this attempt.
    pub receipt: ReceiptStatus,
    /// Event to emit as a side effect of this transition, if any.
    pub emit: Option<EmitSignal>,
}

/// Which terminal cross-crate event the transition should emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmitSignal {
    /// Emit `notify.dispatched` — the message was accepted.
    Dispatched,
    /// Emit `notify.delivery.failed` — retries exhausted.
    Failed,
}

/// Compute the next state after a send attempt, given the prior attempt count and
/// whether the channel accepted the message. This is the bounded retry policy.
///
/// - Success → `Dispatched` (terminal), emit `notify.dispatched`.
/// - Failure with attempts remaining → back to `Pending`, no emit (will retry).
/// - Failure that exhausts [`MAX_DELIVERY_ATTEMPTS`] → `Failed` (terminal),
///   emit `notify.delivery.failed`.
#[must_use]
pub fn apply_attempt(prior_attempts: i32, accepted: bool) -> DispatchTransition {
    let attempts = prior_attempts.saturating_add(1);
    if accepted {
        return DispatchTransition {
            attempts,
            status: OutboxStatus::Dispatched,
            receipt: ReceiptStatus::Delivered,
            emit: Some(EmitSignal::Dispatched),
        };
    }
    if attempts >= MAX_DELIVERY_ATTEMPTS {
        DispatchTransition {
            attempts,
            status: OutboxStatus::Failed,
            receipt: ReceiptStatus::Failed,
            emit: Some(EmitSignal::Failed),
        }
    } else {
        DispatchTransition {
            attempts,
            status: OutboxStatus::Pending,
            receipt: ReceiptStatus::Failed,
            emit: None,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use dsoc_core::ids::{ClusterId, MandateId, ProposalId, SlaId};

    #[test]
    fn channel_kind_roundtrips_through_string() {
        for c in [ChannelKind::Push, ChannelKind::Email, ChannelKind::WhatsApp] {
            assert_eq!(ChannelKind::parse(c.as_str()).unwrap(), c);
        }
    }

    #[test]
    fn channel_kind_rejects_unknown() {
        let err = ChannelKind::parse("carrier-pigeon").unwrap_err();
        assert_eq!(err.code(), "invalid_input");
    }

    #[test]
    fn outbox_status_roundtrips_and_terminality() {
        for s in [
            OutboxStatus::Pending,
            OutboxStatus::Dispatched,
            OutboxStatus::Failed,
        ] {
            assert_eq!(OutboxStatus::parse(s.as_str()).unwrap(), s);
        }
        assert!(!OutboxStatus::Pending.is_terminal());
        assert!(OutboxStatus::Dispatched.is_terminal());
        assert!(OutboxStatus::Failed.is_terminal());
    }

    #[test]
    fn outbox_status_rejects_unknown() {
        assert_eq!(
            OutboxStatus::parse("queued").unwrap_err().code(),
            "invalid_input"
        );
    }

    #[test]
    fn receipt_status_strings_are_stable() {
        assert_eq!(ReceiptStatus::Delivered.as_str(), "delivered");
        assert_eq!(ReceiptStatus::Failed.as_str(), "failed");
    }

    #[test]
    fn device_platform_roundtrips_and_rejects_unknown() {
        for p in [
            DevicePlatform::Ios,
            DevicePlatform::Android,
            DevicePlatform::Web,
        ] {
            assert_eq!(DevicePlatform::parse(p.as_str()).unwrap(), p);
        }
        assert_eq!(
            DevicePlatform::parse("blackberry").unwrap_err().code(),
            "invalid_input"
        );
    }

    #[test]
    fn device_token_input_trims_and_validates() {
        let ok = DeviceTokenInput::parse("ios", "  abc123  ").unwrap();
        assert_eq!(ok.platform, DevicePlatform::Ios);
        assert_eq!(ok.token, "abc123");
    }

    #[test]
    fn device_token_input_rejects_empty() {
        assert_eq!(
            DeviceTokenInput::parse("web", "   ").unwrap_err().code(),
            "invalid_input"
        );
    }

    #[test]
    fn device_token_input_rejects_overlong() {
        let huge = "x".repeat(MAX_DEVICE_TOKEN_LEN + 1);
        assert_eq!(
            DeviceTokenInput::parse("android", &huge)
                .unwrap_err()
                .code(),
            "invalid_input"
        );
    }

    #[test]
    fn device_token_input_rejects_bad_platform() {
        assert_eq!(
            DeviceTokenInput::parse("nokia", "tok").unwrap_err().code(),
            "invalid_input"
        );
    }

    #[test]
    fn plan_for_sla_started_targets_email() {
        let due = Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap();
        let plan = plan_for_event(&Event::ConsequenceSlaStarted {
            sla: SlaId::new(),
            mandate: MandateId::new(),
            cluster: ClusterId::new(),
            due,
        })
        .unwrap();
        assert_eq!(plan.channel, ChannelKind::Email);
        assert_eq!(plan.payload["kind"], "consequence.sla.started");
    }

    #[test]
    fn plan_for_sla_expired_targets_push() {
        let plan = plan_for_event(&Event::ConsequenceSlaExpired {
            sla: SlaId::new(),
            mandate: MandateId::new(),
        })
        .unwrap();
        assert_eq!(plan.channel, ChannelKind::Push);
        assert_eq!(plan.payload["kind"], "consequence.sla.expired");
    }

    #[test]
    fn plan_for_proposal_published_targets_push() {
        let plan = plan_for_event(&Event::ProposalPublished {
            proposal: ProposalId::new(),
            mandate: MandateId::new(),
        })
        .unwrap();
        assert_eq!(plan.channel, ChannelKind::Push);
        assert_eq!(plan.payload["kind"], "proposals.published");
    }

    #[test]
    fn plan_ignores_unhandled_events() {
        assert!(plan_for_event(&Event::VoteCast {
            vote: dsoc_core::ids::VoteId::new(),
            proposal: ProposalId::new(),
        })
        .is_none());
    }

    #[test]
    fn successful_attempt_dispatches_and_emits() {
        let t = apply_attempt(0, true);
        assert_eq!(t.attempts, 1);
        assert_eq!(t.status, OutboxStatus::Dispatched);
        assert_eq!(t.receipt, ReceiptStatus::Delivered);
        assert_eq!(t.emit, Some(EmitSignal::Dispatched));
    }

    #[test]
    fn transient_failure_keeps_pending_without_emit() {
        let t = apply_attempt(0, false);
        assert_eq!(t.attempts, 1);
        assert_eq!(t.status, OutboxStatus::Pending);
        assert_eq!(t.receipt, ReceiptStatus::Failed);
        assert_eq!(t.emit, None);
    }

    #[test]
    fn failure_is_bounded_and_emits_on_exhaustion() {
        // Walk the retry ladder: every attempt below the cap stays pending...
        for prior in 0..(MAX_DELIVERY_ATTEMPTS - 1) {
            assert_eq!(apply_attempt(prior, false).status, OutboxStatus::Pending);
        }
        // ...and the attempt that reaches the cap fails permanently and emits.
        let last = apply_attempt(MAX_DELIVERY_ATTEMPTS - 1, false);
        assert_eq!(last.attempts, MAX_DELIVERY_ATTEMPTS);
        assert_eq!(last.status, OutboxStatus::Failed);
        assert_eq!(last.emit, Some(EmitSignal::Failed));
    }

    #[test]
    fn attempt_count_saturates_and_does_not_overflow() {
        let t = apply_attempt(i32::MAX, false);
        assert_eq!(t.attempts, i32::MAX);
        assert_eq!(t.status, OutboxStatus::Failed);
    }
}

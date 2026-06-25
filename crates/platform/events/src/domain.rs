//! Pure domain types and logic for the durable event bus. No `sqlx`, no `axum`: everything
//! here is deterministic and unit-tested, so the storage and HTTP layers stay thin.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use dsoc_core::events::Event;
use dsoc_core::{Error, Result};

/// Maximum length of a subscriber name (matches the durable column's practical bound).
const MAX_SUBSCRIBER_LEN: usize = 128;

/// Wrap any error as the canonical opaque [`Error::Storage`] (detail logged, never surfaced).
fn storage<E: std::error::Error + Send + Sync + 'static>(err: E) -> Error {
    Error::Storage(Box::new(err))
}

/// An [`Event`] flattened into the columns of `events_log`: its routing `topic`, its stable
/// `event_type` tag, and the full tagged JSON `payload` (which round-trips back to the `Event`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedEvent {
    /// The routing topic string (`EventTopic::as_str`).
    pub topic: &'static str,
    /// The stable serde tag, e.g. `"proposals.created"`.
    pub event_type: String,
    /// The full tagged JSON payload (`{"type":…,"data":…}`), serialized compactly.
    pub payload: String,
}

/// Flatten an [`Event`] into its durable columns.
///
/// # Errors
/// Returns [`Error::Storage`] if the event cannot be serialized or is missing its type tag
/// (neither can happen for the frozen catalog, but the boundary is validated explicitly).
pub fn encode_event(event: &Event) -> Result<EncodedEvent> {
    let value = serde_json::to_value(event).map_err(storage)?;
    let event_type = value
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Storage("event payload missing \"type\" tag".into()))?
        .to_string();
    Ok(EncodedEvent {
        topic: event.topic().as_str(),
        event_type,
        payload: value.to_string(),
    })
}

/// Reconstruct an [`Event`] from a stored JSON `payload`.
///
/// # Errors
/// Returns [`Error::Storage`] if the stored payload is not a valid catalog event (data
/// corruption — never the result of untrusted input, which never reaches this function).
pub fn decode_event(payload: &str) -> Result<Event> {
    serde_json::from_str(payload).map_err(storage)
}

/// A keyset position into `events_log`, ordered by `(occurred_at, id)`. Polling resumes strictly
/// after this point, giving stable, gap-free pagination over an append-only log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    /// The `occurred_at` of the last delivery consumed.
    pub occurred_at: DateTime<Utc>,
    /// The `id` of the last delivery consumed (tie-breaker within the same instant).
    pub id: Uuid,
}

/// A validated subscriber name. Constraining the character set keeps subscriber identities
/// log-safe and avoids ambiguity in the durable cursor table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriberName(String);

impl SubscriberName {
    /// Parse and validate a subscriber name.
    ///
    /// # Errors
    /// Returns [`Error::Validation`] if the name is empty, too long, or contains characters
    /// outside `[A-Za-z0-9_.-]`.
    pub fn parse(raw: &str) -> Result<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(Error::Validation(
                "subscriber name must not be empty".into(),
            ));
        }
        if trimmed.len() > MAX_SUBSCRIBER_LEN {
            return Err(Error::Validation(format!(
                "subscriber name exceeds {MAX_SUBSCRIBER_LEN} characters"
            )));
        }
        if !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
        {
            return Err(Error::Validation(
                "subscriber name may contain only [A-Za-z0-9_.-]".into(),
            ));
        }
        Ok(Self(trimmed.to_string()))
    }

    /// The validated name as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Number of unprocessed deliveries waiting on a single topic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QueueDepth {
    /// The routing topic string.
    pub topic: String,
    /// Count of deliveries with `processed_at IS NULL`.
    pub pending: i64,
}

/// Total pending deliveries across every topic.
#[must_use]
pub fn total_pending(depths: &[QueueDepth]) -> i64 {
    depths.iter().map(|d| d.pending).sum()
}

#[cfg(test)]
mod tests {
    use dsoc_core::events::EventTopic;
    use dsoc_core::ids::{CitizenId, MandateId, ProposalId, VoteId};

    use super::*;

    fn sample_event() -> Event {
        Event::ProposalCreated {
            proposal: ProposalId::new(),
            mandate: MandateId::new(),
        }
    }

    #[test]
    fn encode_extracts_topic_and_tag() {
        let encoded = encode_event(&sample_event()).expect("encode");
        assert_eq!(encoded.topic, "proposals");
        assert_eq!(encoded.event_type, "proposals.created");
        assert!(encoded.payload.contains("\"type\":\"proposals.created\""));
    }

    #[test]
    fn encode_then_decode_roundtrips() {
        let event = sample_event();
        let encoded = encode_event(&event).expect("encode");
        let decoded = decode_event(&encoded.payload).expect("decode");
        assert_eq!(event, decoded);
    }

    #[test]
    fn encode_maps_each_variant_to_its_topic() {
        let vote = Event::VoteCast {
            vote: VoteId::new(),
            proposal: ProposalId::new(),
        };
        assert_eq!(encode_event(&vote).expect("encode").topic, "votes");
        assert_eq!(vote.topic(), EventTopic::Votes);

        let auth = Event::AuthVerificationUpgraded {
            citizen: CitizenId::new(),
        };
        assert_eq!(encode_event(&auth).expect("encode").topic, "auth");
    }

    #[test]
    fn decode_rejects_corrupt_payload() {
        let err = decode_event("not json").expect_err("must fail");
        assert_eq!(err.code(), "storage_error");
    }

    #[test]
    fn decode_rejects_unknown_event_type() {
        let err = decode_event(r#"{"type":"does.not.exist","data":{}}"#).expect_err("must fail");
        assert_eq!(err.code(), "storage_error");
    }

    #[test]
    fn subscriber_name_accepts_safe_identifiers() {
        for name in ["consensus", "notify.worker-1", "audit_chain", "a.b-c_d"] {
            assert_eq!(
                SubscriberName::parse(name).expect("valid").as_str(),
                name,
                "{name} should be accepted"
            );
        }
    }

    #[test]
    fn subscriber_name_trims_surrounding_whitespace() {
        assert_eq!(
            SubscriberName::parse("  worker  ").expect("valid").as_str(),
            "worker"
        );
    }

    #[test]
    fn subscriber_name_rejects_empty() {
        let err = SubscriberName::parse("   ").expect_err("must fail");
        assert_eq!(err.code(), "invalid_input");
    }

    #[test]
    fn subscriber_name_rejects_too_long() {
        let long = "a".repeat(MAX_SUBSCRIBER_LEN + 1);
        let err = SubscriberName::parse(&long).expect_err("must fail");
        assert_eq!(err.code(), "invalid_input");
    }

    #[test]
    fn subscriber_name_rejects_illegal_characters() {
        for bad in ["has space", "drop;table", "emoji😀", "quote\"d"] {
            let err = SubscriberName::parse(bad).expect_err("must fail");
            assert_eq!(err.code(), "invalid_input", "{bad} should be rejected");
        }
    }

    #[test]
    fn total_pending_sums_topics() {
        let depths = vec![
            QueueDepth {
                topic: "votes".into(),
                pending: 3,
            },
            QueueDepth {
                topic: "proposals".into(),
                pending: 4,
            },
        ];
        assert_eq!(total_pending(&depths), 7);
        assert_eq!(total_pending(&[]), 0);
    }

    #[test]
    fn cursor_equality_is_by_occurred_and_id() {
        let t0 = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .expect("parse")
            .with_timezone(&Utc);
        let a = Cursor {
            occurred_at: t0,
            id: Uuid::nil(),
        };
        let b = Cursor {
            occurred_at: t0,
            id: a.id,
        };
        assert_eq!(a, b);
    }
}

//! Pure scorecard domain: value objects, the answered/ignored ledger outcome, the
//! median-latency statistic, and the public projection aggregate. No `sqlx`, no `axum` —
//! everything here is synchronous, side-effect-free, and unit-tested (TESTING.md unit layer).
//!
//! The scorecard owns **no business decisions** (PLAN.md): it is a pure projection of the
//! consequence/mandate event stream. The only logic here is bookkeeping that must be exactly
//! reproducible from the ledger — most importantly, the public counters and the median response
//! latency are derived deterministically from [`Entry`] rows.

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// A value that came from outside the system and failed validation (mapped to
/// [`dsoc_core::Error::Validation`]) or that was read back corrupt from storage.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid scorecard value for {field}: {value}")]
pub struct ParseError {
    /// The logical field that failed to parse (never sensitive).
    pub field: &'static str,
    /// The offending value (echoed back for the caller's diagnostics).
    pub value: String,
}

impl ParseError {
    fn new(field: &'static str, value: impl Into<String>) -> Self {
        Self {
            field,
            value: value.into(),
        }
    }
}

/// The outcome recorded in the ledger for a single consequence SLA.
///
/// An SLA either gets a response within its window ([`Outcome::Answered`], which records a
/// latency) or expires in public silence ([`Outcome::Ignored`], which records none).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The official responded within the SLA window (`consequence.official.responded`).
    Answered,
    /// The SLA expired with no response — public silence (`consequence.sla.expired`).
    Ignored,
}

impl Outcome {
    /// The stable wire/storage token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Outcome::Answered => "answered",
            Outcome::Ignored => "ignored",
        }
    }

    /// Whether this outcome counts as an answer (vs public silence).
    #[must_use]
    pub const fn is_answered(self) -> bool {
        matches!(self, Outcome::Answered)
    }
}

impl FromStr for Outcome {
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "answered" => Ok(Outcome::Answered),
            "ignored" => Ok(Outcome::Ignored),
            other => Err(ParseError::new("outcome", other)),
        }
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The persisted scorecard counters (`scorecard`): the denormalised, public answered/ignored
/// tally. These values are ALWAYS reconstructable from the [`Entry`] ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scorecard {
    /// Scorecard id.
    pub id: Uuid,
    /// Owning organization.
    pub org_id: Uuid,
    /// The mandate (politician) this scorecard belongs to.
    pub mandate_id: Uuid,
    /// Proposals answered within SLA.
    pub answered: i64,
    /// Proposals ignored (public silence).
    pub ignored: i64,
    /// Creation time (from the injected clock).
    pub created_at: DateTime<Utc>,
    /// Last update time (from the injected clock).
    pub updated_at: DateTime<Utc>,
}

/// A single row of the event-sourced ledger (`scorecard_entry`). One row per SLA outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Entry id.
    pub id: Uuid,
    /// The scorecard this entry belongs to.
    pub scorecard_id: Uuid,
    /// The originating SLA — the idempotency dedupe key (UNIQUE in storage).
    pub sla_id: Uuid,
    /// Answered or ignored.
    pub outcome: Outcome,
    /// Response latency in hours (present iff `outcome` is answered).
    pub response_hours: Option<f64>,
    /// When the underlying SLA outcome occurred (the event's time).
    pub occurred_at: DateTime<Utc>,
    /// When this ledger row was written (from the injected clock).
    pub created_at: DateTime<Utc>,
}

/// A persisted public promise (`scorecard_promise`): the "promises vs delivery" half.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Promise {
    /// Promise id.
    pub id: Uuid,
    /// The scorecard this promise belongs to.
    pub scorecard_id: Uuid,
    /// The public promise text (Portuguese civic content).
    pub text: String,
    /// When the promise was made (from the injected clock).
    pub made_at: DateTime<Utc>,
    /// Whether the politician delivered on it.
    pub delivered: bool,
    /// When it was marked delivered (present iff `delivered`).
    pub delivered_at: Option<DateTime<Utc>>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

/// The public projection aggregate that maps onto [`dsoc_api_contract::ScorecardDto`]. Holds the
/// counters plus the derived median response latency.
#[derive(Debug, Clone, PartialEq)]
pub struct ScorecardView {
    /// Scorecard id.
    pub id: Uuid,
    /// Owning organization.
    pub org_id: Uuid,
    /// The mandate (politician) this scorecard belongs to.
    pub mandate_id: Uuid,
    /// Proposals answered within SLA.
    pub answered: i64,
    /// Proposals ignored (public silence).
    pub ignored: i64,
    /// Median response latency across answered entries, in hours (None when none answered).
    pub median_response_hours: Option<f64>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last update time.
    pub updated_at: DateTime<Utc>,
}

impl ScorecardView {
    /// Build the public view from the persisted counters and the answered-entry latencies. The
    /// median is derived here (pure), so the view is always exactly reproducible from the ledger.
    #[must_use]
    pub fn from_parts(card: &Scorecard, answered_hours: &[f64]) -> Self {
        Self {
            id: card.id,
            org_id: card.org_id,
            mandate_id: card.mandate_id,
            answered: card.answered,
            ignored: card.ignored,
            median_response_hours: median_hours(answered_hours),
            created_at: card.created_at,
            updated_at: card.updated_at,
        }
    }
}

/// The median of a set of response latencies in hours, or `None` when empty.
///
/// Pure and deterministic (the heart of the scorecard's latency statistic): for an odd count it is
/// the middle value; for an even count it is the mean of the two central values. NaN inputs sort
/// last and never silently corrupt the ordering.
#[must_use]
pub fn median_hours(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted: Vec<f64> = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Greater));
    let n = sorted.len();
    let mid = n / 2;
    if n % 2 == 1 {
        Some(sorted[mid])
    } else {
        Some((sorted[mid - 1] + sorted[mid]) / 2.0)
    }
}

/// Recount the public counters directly from a ledger slice. This is the invariant the storage
/// layer enforces transactionally: `answered`/`ignored` always equal a fresh recount of the
/// [`Entry`] rows. Exposed so tests can assert the aggregate against the ledger.
#[must_use]
pub fn recount(entries: &[Entry]) -> (i64, i64) {
    let mut answered = 0i64;
    let mut ignored = 0i64;
    for entry in entries {
        match entry.outcome {
            Outcome::Answered => answered += 1,
            Outcome::Ignored => ignored += 1,
        }
    }
    (answered, ignored)
}

/// Validate free-text input at the system boundary (coding-style: validate at boundaries).
/// Trims, rejects empty, and caps length to a sane bound to avoid unbounded storage.
///
/// # Errors
/// Returns [`ParseError`] when the trimmed value is empty or exceeds `max_len`.
pub fn validate_nonempty(
    field: &'static str,
    value: &str,
    max_len: usize,
) -> Result<String, ParseError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() > max_len {
        return Err(ParseError::new(field, value));
    }
    Ok(trimmed.to_owned())
}

/// Validate a response latency at the boundary: it must be finite and non-negative (an SLA cannot
/// be answered in negative time). Maps to [`dsoc_core::Error::Validation`].
///
/// # Errors
/// Returns [`ParseError`] when `hours` is non-finite or negative.
pub fn validate_response_hours(hours: f64) -> Result<f64, ParseError> {
    if hours.is_finite() && hours >= 0.0 {
        Ok(hours)
    } else {
        Err(ParseError::new("response_hours", hours.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(rfc: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(rfc)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn entry(outcome: Outcome, hours: Option<f64>) -> Entry {
        Entry {
            id: Uuid::now_v7(),
            scorecard_id: Uuid::now_v7(),
            sla_id: Uuid::now_v7(),
            outcome,
            response_hours: hours,
            occurred_at: at("2026-06-25T12:00:00Z"),
            created_at: at("2026-06-25T12:00:00Z"),
        }
    }

    #[test]
    fn outcome_roundtrips_and_reports_answered() {
        assert!(Outcome::Answered.is_answered());
        assert!(!Outcome::Ignored.is_answered());
        for o in [Outcome::Answered, Outcome::Ignored] {
            assert_eq!(o.as_str().parse::<Outcome>().unwrap(), o);
            assert_eq!(o.to_string(), o.as_str());
        }
        let err = "maybe".parse::<Outcome>().unwrap_err();
        assert_eq!(err.field, "outcome");
        assert!(err.to_string().contains("maybe"));
    }

    #[test]
    fn median_is_none_when_empty() {
        assert_eq!(median_hours(&[]), None);
    }

    #[test]
    fn median_of_single_value_is_that_value() {
        assert_eq!(median_hours(&[4.5]), Some(4.5));
    }

    #[test]
    fn median_of_odd_count_is_the_middle() {
        // Unsorted input must still yield the correct middle (3.0).
        assert_eq!(median_hours(&[5.0, 1.0, 3.0]), Some(3.0));
    }

    #[test]
    fn median_of_even_count_is_mean_of_two_central() {
        // sorted: [1,2,3,4] -> (2+3)/2 = 2.5
        assert_eq!(median_hours(&[4.0, 1.0, 3.0, 2.0]), Some(2.5));
    }

    #[test]
    fn median_handles_duplicates_and_zero() {
        assert_eq!(median_hours(&[0.0, 0.0, 0.0]), Some(0.0));
        assert_eq!(median_hours(&[2.0, 2.0]), Some(2.0));
    }

    #[test]
    fn recount_matches_ledger() {
        let entries = vec![
            entry(Outcome::Answered, Some(1.0)),
            entry(Outcome::Ignored, None),
            entry(Outcome::Answered, Some(2.0)),
            entry(Outcome::Ignored, None),
            entry(Outcome::Ignored, None),
        ];
        assert_eq!(recount(&entries), (2, 3));
        assert_eq!(recount(&[]), (0, 0));
    }

    #[test]
    fn view_from_parts_derives_median_from_answered_hours() {
        let card = Scorecard {
            id: Uuid::now_v7(),
            org_id: Uuid::now_v7(),
            mandate_id: Uuid::now_v7(),
            answered: 3,
            ignored: 1,
            created_at: at("2026-06-25T12:00:00Z"),
            updated_at: at("2026-06-25T13:00:00Z"),
        };
        let view = ScorecardView::from_parts(&card, &[10.0, 2.0, 6.0]);
        assert_eq!(view.answered, 3);
        assert_eq!(view.ignored, 1);
        assert_eq!(view.median_response_hours, Some(6.0));
        assert_eq!(view.mandate_id, card.mandate_id);
    }

    #[test]
    fn view_median_is_none_with_no_answered_entries() {
        let card = Scorecard {
            id: Uuid::now_v7(),
            org_id: Uuid::now_v7(),
            mandate_id: Uuid::now_v7(),
            answered: 0,
            ignored: 2,
            created_at: at("2026-06-25T12:00:00Z"),
            updated_at: at("2026-06-25T12:00:00Z"),
        };
        let view = ScorecardView::from_parts(&card, &[]);
        assert_eq!(view.median_response_hours, None);
    }

    #[test]
    fn validate_nonempty_trims_and_bounds() {
        assert_eq!(validate_nonempty("text", "  oi  ", 10).unwrap(), "oi");
        assert!(validate_nonempty("text", "   ", 10).is_err());
        assert!(validate_nonempty("text", "toolong", 3).is_err());
        let err = validate_nonempty("text", "", 10).unwrap_err();
        assert_eq!(err.field, "text");
    }

    #[test]
    fn validate_response_hours_rejects_negative_and_nonfinite() {
        assert_eq!(validate_response_hours(0.0).unwrap(), 0.0);
        assert_eq!(validate_response_hours(12.5).unwrap(), 12.5);
        assert!(validate_response_hours(-1.0).is_err());
        assert!(validate_response_hours(f64::NAN).is_err());
        assert!(validate_response_hours(f64::INFINITY).is_err());
    }
}

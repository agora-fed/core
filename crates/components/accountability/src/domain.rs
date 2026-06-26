//! Pure accountability domain: value objects, the bounded progress value, the derived lifecycle
//! status, and the two aggregates. No `sqlx`, no `axum` — everything here is synchronous,
//! side-effect-free, and unit-tested (TESTING.md unit layer).
//!
//! The only logic here is bookkeeping that must be exactly reproducible from input: a result's
//! `status` is DERIVED from its `progress` ([`Status::from_progress`]), so the denormalised marker
//! can never drift from the number it summarises.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Inclusive lower bound of the progress scale.
pub const MIN_PROGRESS: i32 = 0;
/// Inclusive upper bound of the progress scale (a completed commitment).
pub const MAX_PROGRESS: i32 = 100;

/// A value that came from outside the system and failed validation (mapped to
/// [`dsoc_core::Error::Validation`]) or that was read back corrupt from storage.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid accountability value for {field}: {value}")]
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

/// The lifecycle status of a tracked commitment, DERIVED from its progress so the denormalised
/// marker is always reproducible: 0 -> pending, 1..=99 -> in_progress, 100 -> achieved.
/// `Achieved` is terminal — the service refuses further status updates once it is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// No reported progress yet.
    Pending,
    /// Some progress reported, not yet complete.
    InProgress,
    /// Fully delivered (progress at 100). Terminal.
    Achieved,
}

impl Status {
    /// The stable wire/storage token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::InProgress => "in_progress",
            Status::Achieved => "achieved",
        }
    }

    /// Derive the status that corresponds to a (validated) progress value. The single source of
    /// truth for the result/progress consistency invariant the table `CHECK` also enforces.
    #[must_use]
    pub const fn from_progress(progress: i32) -> Self {
        if progress >= MAX_PROGRESS {
            Status::Achieved
        } else if progress <= MIN_PROGRESS {
            Status::Pending
        } else {
            Status::InProgress
        }
    }

    /// Whether this status is terminal (no further updates accepted).
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Status::Achieved)
    }
}

impl FromStr for Status {
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Status::Pending),
            "in_progress" => Ok(Status::InProgress),
            "achieved" => Ok(Status::Achieved),
            other => Err(ParseError::new("status", other)),
        }
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A tracked commitment (`accountability_result`): a titled goal toward a `target`, with bounded
/// progress and the derived lifecycle [`Status`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commitment {
    /// Result id.
    pub id: Uuid,
    /// Owning organization.
    pub org_id: Uuid,
    /// Short public title of the commitment.
    pub title: String,
    /// The target/outcome being committed to (free text).
    pub target: String,
    /// Lifecycle status, always consistent with `progress`.
    pub status: Status,
    /// Progress percentage in `0..=100`.
    pub progress: i32,
    /// Creation time (from the injected clock).
    pub created_at: DateTime<Utc>,
}

/// One row of the append-only progress ledger (`accountability_status_update`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusUpdate {
    /// Status-update id.
    pub id: Uuid,
    /// The result this update belongs to.
    pub result_id: Uuid,
    /// Human note describing what changed (Portuguese civic content).
    pub note: String,
    /// Progress reported at this point, in `0..=100`.
    pub progress: i32,
    /// Creation time (from the injected clock).
    pub created_at: DateTime<Utc>,
}

/// Validate a progress value at the system boundary: it must be within `0..=100`. Maps to
/// [`dsoc_core::Error::Validation`].
///
/// # Errors
/// Returns [`ParseError`] when `progress` is outside the inclusive `0..=100` range.
pub fn validate_progress(progress: i32) -> std::result::Result<i32, ParseError> {
    if (MIN_PROGRESS..=MAX_PROGRESS).contains(&progress) {
        Ok(progress)
    } else {
        Err(ParseError::new("progress", progress.to_string()))
    }
}

/// Validate free-text input at the system boundary (coding-style: validate at boundaries).
/// Trims, rejects empty, and caps length to a sane bound to avoid unbounded storage.
///
/// # Errors
/// Returns [`ParseError`] when the trimmed value is empty or exceeds `max_len` characters.
pub fn validate_nonempty(
    field: &'static str,
    value: &str,
    max_len: usize,
) -> std::result::Result<String, ParseError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() > max_len {
        return Err(ParseError::new(field, value));
    }
    Ok(trimmed.to_owned())
}

/// Validate optional free text that may be empty after trimming (e.g. an open-ended `target`),
/// rejecting only when it exceeds `max_len`.
///
/// # Errors
/// Returns [`ParseError`] when the trimmed value exceeds `max_len` characters.
pub fn validate_bounded(
    field: &'static str,
    value: &str,
    max_len: usize,
) -> std::result::Result<String, ParseError> {
    let trimmed = value.trim();
    if trimmed.chars().count() > max_len {
        return Err(ParseError::new(field, value));
    }
    Ok(trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_roundtrips_through_str() {
        for s in [Status::Pending, Status::InProgress, Status::Achieved] {
            assert_eq!(s.as_str().parse::<Status>().unwrap(), s);
            assert_eq!(s.to_string(), s.as_str());
        }
        let err = "bogus".parse::<Status>().unwrap_err();
        assert_eq!(err.field, "status");
        assert!(err.to_string().contains("bogus"));
    }

    #[test]
    fn status_is_derived_from_progress_buckets() {
        assert_eq!(Status::from_progress(0), Status::Pending);
        assert_eq!(Status::from_progress(1), Status::InProgress);
        assert_eq!(Status::from_progress(50), Status::InProgress);
        assert_eq!(Status::from_progress(99), Status::InProgress);
        assert_eq!(Status::from_progress(100), Status::Achieved);
    }

    #[test]
    fn status_from_progress_clamps_out_of_band_inputs() {
        // Defensive: even if an out-of-range value slips in, the derivation stays total.
        assert_eq!(Status::from_progress(-5), Status::Pending);
        assert_eq!(Status::from_progress(150), Status::Achieved);
    }

    #[test]
    fn only_achieved_is_terminal() {
        assert!(Status::Achieved.is_terminal());
        assert!(!Status::Pending.is_terminal());
        assert!(!Status::InProgress.is_terminal());
    }

    #[test]
    fn validate_progress_bounds_inclusive_range() {
        assert_eq!(validate_progress(0).unwrap(), 0);
        assert_eq!(validate_progress(100).unwrap(), 100);
        assert_eq!(validate_progress(37).unwrap(), 37);
        assert_eq!(validate_progress(-1).unwrap_err().field, "progress");
        assert_eq!(validate_progress(101).unwrap_err().field, "progress");
    }

    #[test]
    fn validate_nonempty_trims_and_bounds() {
        assert_eq!(validate_nonempty("title", "  meta  ", 10).unwrap(), "meta");
        assert!(validate_nonempty("title", "   ", 10).is_err());
        assert!(validate_nonempty("title", "longvalue", 3).is_err());
        let err = validate_nonempty("title", "", 10).unwrap_err();
        assert_eq!(err.field, "title");
    }

    #[test]
    fn validate_bounded_allows_empty_but_caps_length() {
        assert_eq!(validate_bounded("target", "   ", 10).unwrap(), "");
        assert_eq!(validate_bounded("target", " alvo ", 10).unwrap(), "alvo");
        assert!(validate_bounded("target", "toolong", 3).is_err());
    }
}

//! Pure vote domain: the privacy-preserving aggregate type and the small value-logic the
//! service and HTTP layers build on. No `sqlx`, no `axum` — every function here is
//! deterministic and unit-tested (TESTING.md). The defining invariant of this crate lives
//! here: the official-facing aggregate ([`TallyView`]) carries **no citizen linkage by
//! construction**, so a citizen id cannot leak into an official response (LGPD; PLAN.md DO-NOT).

use chrono::{DateTime, Utc};

use dsoc_core::ids::ProposalId;
use dsoc_core::VerificationLevel;

/// The minimum verification level required to cast a support signal. Anonymous visitors cannot
/// vote; an email-confirmed citizen is the floor (CRATE spec). The HTTP layer enforces this via
/// the injected [`dsoc_core::Authorization`] before any write.
pub const MIN_VOTE_LEVEL: VerificationLevel = VerificationLevel::Email;

/// The privacy-safe aggregate an official may read for a proposal. It is intentionally a closed
/// shape over `(proposal, support_count, updated_at)`: there is **no** field that could carry a
/// citizen id, so the type system itself prevents the protected linkage from reaching an official.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TallyView {
    /// The proposal the aggregate is for.
    pub proposal: ProposalId,
    /// Distinct supporters — a count only, never a list of who.
    pub support_count: u64,
    /// When the aggregate last changed (from the injected clock).
    pub updated_at: DateTime<Utc>,
}

/// Normalize a raw `bigint` support count read from PostgreSQL into a non-negative `u64`. A
/// negative value is impossible under the `CHECK (support_count >= 0)` constraint, but a corrupt
/// row clamps to zero rather than panicking or wrapping.
#[must_use]
pub const fn normalize_support(raw: i64) -> u64 {
    if raw < 0 {
        0
    } else {
        raw as u64
    }
}

/// Whether a citizen at `actual` assurance may cast a support signal. Pure mirror of the
/// authorization gate (the live check goes through [`dsoc_core::Authorization`]); kept here so the
/// rule is unit-testable without a database.
#[must_use]
pub fn can_vote(actual: VerificationLevel) -> bool {
    actual >= MIN_VOTE_LEVEL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anonymous_cannot_vote_email_and_above_can() {
        assert!(!can_vote(VerificationLevel::Anonymous));
        assert!(can_vote(VerificationLevel::Email));
        assert!(can_vote(VerificationLevel::Directory));
        assert!(can_vote(VerificationLevel::Strong));
    }

    #[test]
    fn min_vote_level_is_email() {
        assert_eq!(MIN_VOTE_LEVEL, VerificationLevel::Email);
    }

    #[test]
    fn normalize_support_clamps_negatives_and_passes_through() {
        assert_eq!(normalize_support(0), 0);
        assert_eq!(normalize_support(42), 42);
        assert_eq!(normalize_support(-1), 0);
        assert_eq!(normalize_support(i64::MAX), i64::MAX as u64);
    }

    #[test]
    fn tally_view_has_no_citizen_field() {
        // Documentation-by-construction: a `TallyView` is built from only these three fields.
        // If a citizen field were ever added, this literal would fail to compile — the
        // privacy invariant is enforced by the type, not by reviewer vigilance.
        let view = TallyView {
            proposal: ProposalId::new(),
            support_count: 3,
            updated_at: Utc::now(),
        };
        assert_eq!(view.support_count, 3);
    }
}

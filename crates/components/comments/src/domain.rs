//! Pure comments domain: value objects, the thread-depth guard, the comment status
//! lifecycle, and the up/down vote weight. No `sqlx`, no `axum` — everything here is
//! synchronous, side-effect-free, and unit-tested (TESTING.md unit layer).
//!
//! The two rules that matter for deliberation integrity live here so they are provable in
//! isolation: (1) a reply may not exceed [`MAX_THREAD_DEPTH`] (bounded threads), and
//! (2) a comment's status only moves forward `visible -> flagged -> hidden`, so a
//! moderation signal can never silently un-hide content.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Maximum nesting depth of a reply. A root comment is depth `0`; the deepest reply a
/// caller may create has depth `MAX_THREAD_DEPTH`. Bounding depth keeps threads
/// renderable and protects against pathological recursion.
pub const MAX_THREAD_DEPTH: i32 = 6;

/// Hard cap on a comment body, in characters (validated at the boundary).
pub const MAX_BODY_LEN: usize = 10_000;

/// A value that arrived from outside the system and failed validation (mapped to
/// [`dsoc_core::Error::Validation`]) or that was read back corrupt from storage.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid comment value for {field}: {value}")]
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

/// A reply would nest deeper than [`MAX_THREAD_DEPTH`] (mapped to
/// [`dsoc_core::Error::Conflict`] — a violated domain rule, not malformed input).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("thread too deep: reply depth {attempted} exceeds maximum {max}")]
pub struct DepthError {
    /// The depth the rejected reply would have had.
    pub attempted: i32,
    /// The maximum permitted depth.
    pub max: i32,
}

/// Lifecycle of a comment: `Visible -> Flagged -> Hidden`. Moderation only ever moves a
/// comment toward less visibility; it can never be silently restored, so a flag/hide is a
/// durable, auditable signal. Already being at (or past) the target is a no-op, which keeps
/// the moderation consumer idempotent under at-least-once delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentStatus {
    /// Default — the comment is part of the public thread.
    Visible,
    /// Flagged by moderation; kept visible but marked for review.
    Flagged,
    /// Hidden by moderation; withheld from the public thread.
    Hidden,
}

impl CommentStatus {
    /// The stable wire/storage token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            CommentStatus::Visible => "visible",
            CommentStatus::Flagged => "flagged",
            CommentStatus::Hidden => "hidden",
        }
    }

    /// Monotonic rank used to decide whether a moderation transition moves content
    /// strictly toward less visibility.
    const fn rank(self) -> u8 {
        match self {
            CommentStatus::Visible => 0,
            CommentStatus::Flagged => 1,
            CommentStatus::Hidden => 2,
        }
    }

    /// Whether a moderation transition from `self` to `to` is a real forward move. Returns
    /// `false` when `to` is the same or more visible than `self` (an idempotent no-op),
    /// so the consumer can treat a redundant moderation signal as already-applied.
    #[must_use]
    pub const fn advances_to(self, to: CommentStatus) -> bool {
        to.rank() > self.rank()
    }
}

impl FromStr for CommentStatus {
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "visible" => Ok(CommentStatus::Visible),
            "flagged" => Ok(CommentStatus::Flagged),
            "hidden" => Ok(CommentStatus::Hidden),
            other => Err(ParseError::new("status", other)),
        }
    }
}

impl fmt::Display for CommentStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A citizen's up/down weight on a comment. Constrained to the unit set `{-1, +1}` — the
/// same constraint the `comment_vote.weight` CHECK enforces in storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoteWeight {
    /// Upvote (`+1`).
    Up,
    /// Downvote (`-1`).
    Down,
}

impl VoteWeight {
    /// The stored `smallint` value (`+1` or `-1`).
    #[must_use]
    pub const fn as_i16(self) -> i16 {
        match self {
            VoteWeight::Up => 1,
            VoteWeight::Down => -1,
        }
    }

    /// Parse a raw weight, accepting only `+1` or `-1`.
    ///
    /// # Errors
    /// Returns [`ParseError`] for any value other than `1` or `-1`.
    pub fn from_i16(raw: i16) -> Result<Self, ParseError> {
        match raw {
            1 => Ok(VoteWeight::Up),
            -1 => Ok(VoteWeight::Down),
            other => Err(ParseError::new("weight", other.to_string())),
        }
    }
}

/// The depth a new node receives given its parent (if any). A root (no parent) is depth
/// `0`; a reply is `parent_depth + 1`, rejected when it would exceed [`MAX_THREAD_DEPTH`].
///
/// This is the whole thread-depth guard, kept pure so it is provable without a database.
///
/// # Errors
/// Returns [`DepthError`] when a reply would nest deeper than [`MAX_THREAD_DEPTH`].
pub fn child_depth(parent_depth: Option<i32>) -> Result<i32, DepthError> {
    match parent_depth {
        None => Ok(0),
        Some(d) => {
            let attempted = d.saturating_add(1);
            if attempted > MAX_THREAD_DEPTH {
                Err(DepthError {
                    attempted,
                    max: MAX_THREAD_DEPTH,
                })
            } else {
                Ok(attempted)
            }
        }
    }
}

/// Validate a comment body at the system boundary (coding-style: validate at boundaries).
/// Trims surrounding whitespace, rejects an empty body, and caps the length.
///
/// # Errors
/// Returns [`ParseError`] when the trimmed body is empty or exceeds [`MAX_BODY_LEN`].
pub fn validate_body(body: &str) -> Result<String, ParseError> {
    let trimmed = body.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_BODY_LEN {
        return Err(ParseError::new("body", body));
    }
    Ok(trimmed.to_owned())
}

/// A persisted comment (`comment`): one node in a deliberation thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    /// Comment id.
    pub id: Uuid,
    /// Owning organization.
    pub org_id: Uuid,
    /// The votable entity (proposal) this thread hangs off.
    pub proposal_id: Uuid,
    /// Parent comment, or `None` for a root.
    pub parent_id: Option<Uuid>,
    /// Author (a `citizen`).
    pub author_id: Uuid,
    /// The comment text (Portuguese civic content).
    pub body: String,
    /// Nesting depth (0 for a root).
    pub depth: i32,
    /// Lifecycle status.
    pub status: CommentStatus,
    /// Creation time (from the injected clock).
    pub created_at: DateTime<Utc>,
}

/// A persisted vote (`comment_vote`): one citizen's weight on one comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentVote {
    /// Vote id (the REAL stored id, even on an idempotent re-vote).
    pub id: Uuid,
    /// The comment voted on.
    pub comment_id: Uuid,
    /// The voting citizen.
    pub citizen_id: Uuid,
    /// The up/down weight.
    pub weight: i16,
    /// Creation time of the original vote (unchanged by a re-vote).
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_status_roundtrips() {
        for s in [
            CommentStatus::Visible,
            CommentStatus::Flagged,
            CommentStatus::Hidden,
        ] {
            assert_eq!(s.as_str().parse::<CommentStatus>().unwrap(), s);
            assert_eq!(s.to_string(), s.as_str());
        }
        let err = "deleted".parse::<CommentStatus>().unwrap_err();
        assert_eq!(err.field, "status");
        assert!(err.to_string().contains("deleted"));
    }

    #[test]
    fn status_only_advances_toward_less_visibility() {
        // Forward moves are real transitions.
        assert!(CommentStatus::Visible.advances_to(CommentStatus::Flagged));
        assert!(CommentStatus::Visible.advances_to(CommentStatus::Hidden));
        assert!(CommentStatus::Flagged.advances_to(CommentStatus::Hidden));
        // Same or more-visible targets are idempotent no-ops (never un-hide).
        assert!(!CommentStatus::Visible.advances_to(CommentStatus::Visible));
        assert!(!CommentStatus::Flagged.advances_to(CommentStatus::Flagged));
        assert!(!CommentStatus::Flagged.advances_to(CommentStatus::Visible));
        assert!(!CommentStatus::Hidden.advances_to(CommentStatus::Flagged));
        assert!(!CommentStatus::Hidden.advances_to(CommentStatus::Visible));
    }

    #[test]
    fn vote_weight_roundtrips_unit_set() {
        assert_eq!(VoteWeight::Up.as_i16(), 1);
        assert_eq!(VoteWeight::Down.as_i16(), -1);
        assert_eq!(VoteWeight::from_i16(1).unwrap(), VoteWeight::Up);
        assert_eq!(VoteWeight::from_i16(-1).unwrap(), VoteWeight::Down);
    }

    #[test]
    fn vote_weight_rejects_values_outside_unit_set() {
        for bad in [0i16, 2, -2, 100] {
            let err = VoteWeight::from_i16(bad).unwrap_err();
            assert_eq!(err.field, "weight");
        }
    }

    #[test]
    fn root_comment_is_depth_zero() {
        assert_eq!(child_depth(None).unwrap(), 0);
    }

    #[test]
    fn reply_is_one_deeper_than_its_parent() {
        assert_eq!(child_depth(Some(0)).unwrap(), 1);
        assert_eq!(child_depth(Some(3)).unwrap(), 4);
    }

    #[test]
    fn reply_at_the_limit_is_allowed_but_one_beyond_is_rejected() {
        // A parent at MAX-1 yields a child at exactly MAX (allowed).
        let deepest = child_depth(Some(MAX_THREAD_DEPTH - 1)).unwrap();
        assert_eq!(deepest, MAX_THREAD_DEPTH);
        // A parent already at MAX would push the child past the limit.
        let err = child_depth(Some(MAX_THREAD_DEPTH)).unwrap_err();
        assert_eq!(err.attempted, MAX_THREAD_DEPTH + 1);
        assert_eq!(err.max, MAX_THREAD_DEPTH);
        assert!(err.to_string().contains("thread too deep"));
    }

    #[test]
    fn depth_guard_saturates_instead_of_overflowing() {
        // A corrupt/extreme parent depth must not panic on overflow; it is rejected.
        let err = child_depth(Some(i32::MAX)).unwrap_err();
        assert_eq!(err.attempted, i32::MAX);
    }

    #[test]
    fn validate_body_trims_and_bounds() {
        assert_eq!(validate_body("  olá mundo  ").unwrap(), "olá mundo");
        assert!(validate_body("   ").is_err());
        assert!(validate_body("").is_err());
        let toolong = "a".repeat(MAX_BODY_LEN + 1);
        assert!(validate_body(&toolong).is_err());
        // Exactly at the cap is accepted.
        let atcap = "a".repeat(MAX_BODY_LEN);
        assert_eq!(validate_body(&atcap).unwrap().chars().count(), MAX_BODY_LEN);
    }
}

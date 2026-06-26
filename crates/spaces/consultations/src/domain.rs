//! Pure consultation domain: the lifecycle status, the boundary-validation rules, and the
//! view types the service and HTTP layers build on. No `sqlx`, no `axum` — every function here
//! is deterministic and unit-tested (TESTING.md). The state machine the rest of the crate
//! guards (`Open -> Closed`, never the reverse) is defined here.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use dsoc_core::ids::{OrgId, SpaceId};
use dsoc_core::VerificationLevel;

/// The minimum verification level required to manage a consultation (create / close). Putting a
/// scoped question to the population is a stewardship action, so the floor is `Directory`
/// (a verified, directory-listed actor) — above an ordinary email-confirmed citizen. The HTTP
/// layer enforces this through the injected [`dsoc_core::Authorization`] before any write.
pub const MIN_MANAGE_LEVEL: VerificationLevel = VerificationLevel::Directory;

/// Maximum length of a consultation title (characters). Bounds the stored text and the payload.
pub const MAX_TITLE_LEN: usize = 200;

/// Maximum length of a single question prompt (characters).
pub const MAX_PROMPT_LEN: usize = 500;

/// Maximum number of questions a single consultation may carry (bounds the create payload).
pub const MAX_QUESTIONS: usize = 50;

/// The lifecycle state of a consultation. A closed enum (mirrors the SQL `CHECK`): a consultation
/// is created `Open` for participation and transitions once to `Closed`. There is no reopen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsultationStatus {
    /// Accepting participation within its response window.
    Open,
    /// Permanently closed; the window has been ended.
    Closed,
}

impl ConsultationStatus {
    /// The stable database/wire string for this status.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            ConsultationStatus::Open => "open",
            ConsultationStatus::Closed => "closed",
        }
    }

    /// Parse a status read from the database. Returns `None` for any value the `CHECK` forbids
    /// (a corrupt row), so the caller maps it to a storage error rather than guessing.
    #[must_use]
    pub fn from_db(raw: &str) -> Option<Self> {
        match raw {
            "open" => Some(ConsultationStatus::Open),
            "closed" => Some(ConsultationStatus::Closed),
            _ => None,
        }
    }

    /// Whether this status still accepts participation.
    #[must_use]
    pub const fn is_open(self) -> bool {
        matches!(self, ConsultationStatus::Open)
    }
}

/// Whether a citizen at `actual` assurance may create or close a consultation. Pure mirror of the
/// authorization gate (the live check goes through [`dsoc_core::Authorization`]); kept here so the
/// rule is unit-testable without a database.
#[must_use]
pub fn can_manage(actual: VerificationLevel) -> bool {
    actual >= MIN_MANAGE_LEVEL
}

/// Whether a consultation title is acceptable: non-blank once trimmed and within [`MAX_TITLE_LEN`].
#[must_use]
pub fn is_valid_title(title: &str) -> bool {
    let trimmed = title.trim();
    !trimmed.is_empty() && trimmed.chars().count() <= MAX_TITLE_LEN
}

/// Whether a question prompt is acceptable: non-blank once trimmed and within [`MAX_PROMPT_LEN`].
#[must_use]
pub fn is_valid_prompt(prompt: &str) -> bool {
    let trimmed = prompt.trim();
    !trimmed.is_empty() && trimmed.chars().count() <= MAX_PROMPT_LEN
}

/// Whether a response window is coherent: it opens strictly before it closes (mirrors the SQL
/// `CHECK (opens_at < closes_at)`).
#[must_use]
pub fn is_valid_window(opens_at: DateTime<Utc>, closes_at: DateTime<Utc>) -> bool {
    opens_at < closes_at
}

/// Whether a question count is within the allowed bounds: at least one, at most [`MAX_QUESTIONS`].
#[must_use]
pub const fn is_valid_question_count(count: usize) -> bool {
    count >= 1 && count <= MAX_QUESTIONS
}

/// A consultation as the domain sees it (mapped to/from rows and DTOs at the boundaries).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsultationView {
    /// The consultation's identity — a participation space instance.
    pub id: SpaceId,
    /// The organization/tenant the consultation belongs to.
    pub org: OrgId,
    /// The human title of the consultation.
    pub title: String,
    /// When the response window opens.
    pub opens_at: DateTime<Utc>,
    /// When the response window closes.
    pub closes_at: DateTime<Utc>,
    /// The lifecycle status.
    pub status: ConsultationStatus,
    /// When the consultation was created (from the injected clock).
    pub created_at: DateTime<Utc>,
}

/// A single question within a consultation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionView {
    /// The question's opaque id.
    pub id: Uuid,
    /// The question text shown to participants (Portuguese — civic content policy).
    pub prompt: String,
    /// 0-based display order within the consultation.
    pub position: i32,
    /// When the question was created (from the injected clock).
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        #![allow(clippy::unwrap_used)]
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn status_roundtrips_through_db_strings() {
        assert_eq!(ConsultationStatus::Open.as_str(), "open");
        assert_eq!(ConsultationStatus::Closed.as_str(), "closed");
        assert_eq!(
            ConsultationStatus::from_db("open"),
            Some(ConsultationStatus::Open)
        );
        assert_eq!(
            ConsultationStatus::from_db("closed"),
            Some(ConsultationStatus::Closed)
        );
        assert_eq!(ConsultationStatus::from_db("paused"), None);
    }

    #[test]
    fn open_is_open_closed_is_not() {
        assert!(ConsultationStatus::Open.is_open());
        assert!(!ConsultationStatus::Closed.is_open());
    }

    #[test]
    fn only_directory_and_above_may_manage() {
        assert!(!can_manage(VerificationLevel::Anonymous));
        assert!(!can_manage(VerificationLevel::Email));
        assert!(can_manage(VerificationLevel::Directory));
        assert!(can_manage(VerificationLevel::Strong));
        assert_eq!(MIN_MANAGE_LEVEL, VerificationLevel::Directory);
    }

    #[test]
    fn title_validation_rejects_blank_and_overlong() {
        assert!(is_valid_title("Orçamento participativo 2026"));
        assert!(!is_valid_title(""));
        assert!(!is_valid_title("   "));
        assert!(is_valid_title(&"a".repeat(MAX_TITLE_LEN)));
        assert!(!is_valid_title(&"a".repeat(MAX_TITLE_LEN + 1)));
    }

    #[test]
    fn prompt_validation_rejects_blank_and_overlong() {
        assert!(is_valid_prompt("Você apoia a ciclovia na Av. Central?"));
        assert!(!is_valid_prompt("  "));
        assert!(is_valid_prompt(&"q".repeat(MAX_PROMPT_LEN)));
        assert!(!is_valid_prompt(&"q".repeat(MAX_PROMPT_LEN + 1)));
    }

    #[test]
    fn window_must_open_before_it_closes() {
        let opens = at("2026-07-01T00:00:00Z");
        let closes = at("2026-07-08T00:00:00Z");
        assert!(is_valid_window(opens, closes));
        assert!(!is_valid_window(closes, opens));
        assert!(!is_valid_window(opens, opens));
    }

    #[test]
    fn question_count_bounds() {
        assert!(!is_valid_question_count(0));
        assert!(is_valid_question_count(1));
        assert!(is_valid_question_count(MAX_QUESTIONS));
        assert!(!is_valid_question_count(MAX_QUESTIONS + 1));
    }
}

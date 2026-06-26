//! Pure meetings domain: boundary validation for scheduling a meeting and for the minutes
//! recorded against it. No `sqlx`, no `axum`: everything here is synchronous, side-effect-free,
//! and unit-tested (TESTING.md unit layer).
//!
//! Keeping validation here means the rules that protect a meeting — a title and a location are
//! always present and bounded, and minutes (when set) are non-empty and bounded — are provable
//! without a database. The meeting's `starts_at` instant is a plain timestamp carried through
//! unchanged; it is not constrained here (a meeting may be scheduled in the past or future).

use chrono::{DateTime, Utc};
use dsoc_core::{Error, Result};

/// Maximum length of a meeting title.
pub const MAX_TITLE_LEN: usize = 200;
/// Maximum length of a meeting location (room, address, or online URL).
pub const MAX_LOCATION_LEN: usize = 500;
/// Maximum length of the recorded minutes.
pub const MAX_MINUTES_LEN: usize = 50_000;

/// A validated meeting-scheduling request (title/location checked & trimmed at the boundary;
/// `starts_at` carried through unchanged).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMeeting {
    /// Title — what the meeting is about (Portuguese civic content).
    pub title: String,
    /// Location — where it happens (a room, an address, or an online URL).
    pub location: String,
    /// When the meeting begins.
    pub starts_at: DateTime<Utc>,
}

impl NewMeeting {
    /// Validate and normalise raw schedule input (trim, non-empty, length bounds).
    ///
    /// # Errors
    /// [`Error::Validation`] when the title/location are empty or over their length bounds.
    pub fn validate(title: &str, location: &str, starts_at: DateTime<Utc>) -> Result<Self> {
        let title = title.trim();
        let location = location.trim();
        if title.is_empty() {
            return Err(Error::Validation("title must not be empty".to_string()));
        }
        if location.is_empty() {
            return Err(Error::Validation("location must not be empty".to_string()));
        }
        if title.chars().count() > MAX_TITLE_LEN {
            return Err(Error::Validation(format!(
                "title must be at most {MAX_TITLE_LEN} characters"
            )));
        }
        if location.chars().count() > MAX_LOCATION_LEN {
            return Err(Error::Validation(format!(
                "location must be at most {MAX_LOCATION_LEN} characters"
            )));
        }
        Ok(Self {
            title: title.to_string(),
            location: location.to_string(),
            starts_at,
        })
    }
}

/// Validated minutes text for a meeting (non-empty and bounded after trimming).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidMinutes(String);

impl ValidMinutes {
    /// Validate and normalise raw minutes input (trim, non-empty, length bound).
    ///
    /// # Errors
    /// [`Error::Validation`] when the minutes are empty or over [`MAX_MINUTES_LEN`].
    pub fn validate(minutes: &str) -> Result<Self> {
        let minutes = minutes.trim();
        if minutes.is_empty() {
            return Err(Error::Validation("minutes must not be empty".to_string()));
        }
        if minutes.chars().count() > MAX_MINUTES_LEN {
            return Err(Error::Validation(format!(
                "minutes must be at most {MAX_MINUTES_LEN} characters"
            )));
        }
        Ok(Self(minutes.to_string()))
    }

    /// The validated minutes text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 25, 18, 30, 0).unwrap()
    }

    #[test]
    fn new_meeting_trims_and_accepts_content() {
        let m =
            NewMeeting::validate("  Assembleia do bairro  ", "  Praça Central  ", at()).unwrap();
        assert_eq!(m.title, "Assembleia do bairro");
        assert_eq!(m.location, "Praça Central");
        assert_eq!(m.starts_at, at());
    }

    #[test]
    fn new_meeting_rejects_blank_title() {
        let err = NewMeeting::validate("   ", "local", at()).unwrap_err();
        assert_eq!(err.code(), "invalid_input");
    }

    #[test]
    fn new_meeting_rejects_blank_location() {
        let err = NewMeeting::validate("título", "   ", at()).unwrap_err();
        assert_eq!(err.code(), "invalid_input");
    }

    #[test]
    fn new_meeting_rejects_overlong_title() {
        let long = "a".repeat(MAX_TITLE_LEN + 1);
        assert!(NewMeeting::validate(&long, "local", at()).is_err());
    }

    #[test]
    fn new_meeting_rejects_overlong_location() {
        let long = "a".repeat(MAX_LOCATION_LEN + 1);
        assert!(NewMeeting::validate("título", &long, at()).is_err());
    }

    #[test]
    fn minutes_trim_and_accept_content() {
        let m = ValidMinutes::validate("  decidimos avançar  ").unwrap();
        assert_eq!(m.as_str(), "decidimos avançar");
    }

    #[test]
    fn minutes_reject_blank() {
        let err = ValidMinutes::validate("   ").unwrap_err();
        assert_eq!(err.code(), "invalid_input");
    }

    #[test]
    fn minutes_reject_overlong() {
        let long = "a".repeat(MAX_MINUTES_LEN + 1);
        assert!(ValidMinutes::validate(&long).is_err());
    }
}

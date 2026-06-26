//! Pure debates domain: the pro/con/neutral [`Stance`] value object and the boundary
//! validation for a new debate and a new contribution. No `sqlx`, no `axum`: everything
//! here is synchronous, side-effect-free, and unit-tested (TESTING.md unit layer).
//!
//! Keeping validation here means the two rules that protect a debate space — a stance is
//! always one of the three allowed tokens, and titles/framings/bodies are trimmed and
//! bounded — are provable without a database.

use dsoc_core::{Error, Result};

/// Maximum length of a debate title (the motion under debate).
pub const MAX_TITLE_LEN: usize = 200;
/// Maximum length of a debate framing (the neutral context).
pub const MAX_FRAMING_LEN: usize = 20_000;
/// Maximum length of a contribution body.
pub const MAX_BODY_LEN: usize = 20_000;

/// A position taken in a debate. Constrained to the same closed set the
/// `debate_contribution.stance` CHECK enforces in storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stance {
    /// In favour of the motion.
    Pro,
    /// Against the motion.
    Con,
    /// Neither for nor against (a qualifying or undecided contribution).
    Neutral,
}

impl Stance {
    /// The stable text stored in the `debate_contribution.stance` column (matches the SQL
    /// CHECK constraint).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Stance::Pro => "pro",
            Stance::Con => "con",
            Stance::Neutral => "neutral",
        }
    }

    /// Pure lookup of a stance token; `None` for any value outside the closed set.
    #[must_use]
    fn lookup(s: &str) -> Option<Self> {
        match s {
            "pro" => Some(Stance::Pro),
            "con" => Some(Stance::Con),
            "neutral" => Some(Stance::Neutral),
            _ => None,
        }
    }

    /// Parse a stance arriving from outside the system (request input).
    ///
    /// # Errors
    /// [`Error::Validation`] for any value other than `pro` / `con` / `neutral`.
    pub fn parse_input(s: &str) -> Result<Self> {
        Self::lookup(s.trim())
            .ok_or_else(|| Error::Validation(format!("stance must be pro, con or neutral: {s}")))
    }

    /// Parse a stance read back from storage. A bad value here is a schema/CHECK drift bug,
    /// not client input, so it surfaces as an internal [`Error::Storage`].
    ///
    /// # Errors
    /// [`Error::Storage`] for an unrecognised stored value.
    pub fn parse_stored(s: &str) -> Result<Self> {
        Self::lookup(s).ok_or_else(|| Error::Storage(format!("unknown stance: {s}").into()))
    }
}

/// A validated debate creation request (title/framing checked at the boundary).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDebate {
    /// Title — the motion/question under debate (Portuguese civic content).
    pub title: String,
    /// Framing — the neutral context that frames the pro/con discussion.
    pub framing: String,
}

impl NewDebate {
    /// Validate and normalise raw create input (trim, non-empty, length bounds).
    ///
    /// # Errors
    /// [`Error::Validation`] when the title/framing are empty or over their length bounds.
    pub fn validate(title: &str, framing: &str) -> Result<Self> {
        let title = title.trim();
        let framing = framing.trim();
        if title.is_empty() {
            return Err(Error::Validation("title must not be empty".to_string()));
        }
        if framing.is_empty() {
            return Err(Error::Validation("framing must not be empty".to_string()));
        }
        if title.chars().count() > MAX_TITLE_LEN {
            return Err(Error::Validation(format!(
                "title must be at most {MAX_TITLE_LEN} characters"
            )));
        }
        if framing.chars().count() > MAX_FRAMING_LEN {
            return Err(Error::Validation(format!(
                "framing must be at most {MAX_FRAMING_LEN} characters"
            )));
        }
        Ok(Self {
            title: title.to_string(),
            framing: framing.to_string(),
        })
    }
}

/// A validated contribution to a debate (stance + body checked at the boundary).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewContribution {
    /// The stance this contribution takes.
    pub stance: Stance,
    /// The contribution text (Portuguese civic content).
    pub body: String,
}

impl NewContribution {
    /// Validate and normalise a raw contribution (parse the stance, trim & bound the body).
    ///
    /// # Errors
    /// [`Error::Validation`] when the stance is not one of `pro`/`con`/`neutral`, or the body
    /// is empty or over [`MAX_BODY_LEN`].
    pub fn validate(stance: &str, body: &str) -> Result<Self> {
        let stance = Stance::parse_input(stance)?;
        let body = body.trim();
        if body.is_empty() {
            return Err(Error::Validation("body must not be empty".to_string()));
        }
        if body.chars().count() > MAX_BODY_LEN {
            return Err(Error::Validation(format!(
                "body must be at most {MAX_BODY_LEN} characters"
            )));
        }
        Ok(Self {
            stance,
            body: body.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stance_round_trips_through_text() {
        for s in [Stance::Pro, Stance::Con, Stance::Neutral] {
            assert_eq!(Stance::parse_input(s.as_str()).unwrap(), s);
            assert_eq!(Stance::parse_stored(s.as_str()).unwrap(), s);
        }
    }

    #[test]
    fn stance_parse_input_trims_and_rejects_unknown() {
        assert_eq!(Stance::parse_input("  pro  ").unwrap(), Stance::Pro);
        let err = Stance::parse_input("maybe").unwrap_err();
        assert_eq!(err.code(), "invalid_input");
    }

    #[test]
    fn stance_parse_stored_rejects_unknown_as_storage() {
        let err = Stance::parse_stored("bogus").unwrap_err();
        assert_eq!(err.code(), "storage_error");
    }

    #[test]
    fn new_debate_trims_and_accepts_content() {
        let d =
            NewDebate::validate("  Transporte gratuito?  ", "  Debate sobre tarifa zero ").unwrap();
        assert_eq!(d.title, "Transporte gratuito?");
        assert_eq!(d.framing, "Debate sobre tarifa zero");
    }

    #[test]
    fn new_debate_rejects_blank_title() {
        let err = NewDebate::validate("   ", "framing").unwrap_err();
        assert_eq!(err.code(), "invalid_input");
    }

    #[test]
    fn new_debate_rejects_blank_framing() {
        assert!(NewDebate::validate("title", "   ").is_err());
    }

    #[test]
    fn new_debate_rejects_overlong_title() {
        let long = "a".repeat(MAX_TITLE_LEN + 1);
        assert!(NewDebate::validate(&long, "framing").is_err());
    }

    #[test]
    fn new_debate_rejects_overlong_framing() {
        let long = "a".repeat(MAX_FRAMING_LEN + 1);
        assert!(NewDebate::validate("title", &long).is_err());
    }

    #[test]
    fn new_contribution_validates_stance_and_body() {
        let c = NewContribution::validate("con", "  discordo plenamente ").unwrap();
        assert_eq!(c.stance, Stance::Con);
        assert_eq!(c.body, "discordo plenamente");
    }

    #[test]
    fn new_contribution_rejects_bad_stance() {
        let err = NewContribution::validate("sim", "corpo").unwrap_err();
        assert_eq!(err.code(), "invalid_input");
    }

    #[test]
    fn new_contribution_rejects_blank_body() {
        assert!(NewContribution::validate("pro", "   ").is_err());
    }

    #[test]
    fn new_contribution_rejects_overlong_body() {
        let long = "a".repeat(MAX_BODY_LEN + 1);
        assert!(NewContribution::validate("neutral", &long).is_err());
    }
}

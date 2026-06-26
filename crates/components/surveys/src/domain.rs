//! Pure surveys domain: the typed [`QuestionKind`] value object and the boundary
//! validation for a new survey, a new question, and a citizen response. No `sqlx`, no
//! `axum`: everything here is synchronous, side-effect-free, and unit-tested (TESTING.md
//! unit layer).
//!
//! Keeping validation here means the rules that protect a survey — a question kind is
//! always one of the allowed tokens, titles/prompts are trimmed and bounded, a position
//! is non-negative, and a response is a JSON object — are provable without a database.

use dsoc_core::{Error, Result};

/// Maximum length of a survey title.
pub const MAX_TITLE_LEN: usize = 200;
/// Maximum length of a question prompt.
pub const MAX_PROMPT_LEN: usize = 2_000;
/// Maximum number of top-level answer entries accepted in a single response (keeps a
/// response bounded; the questionnaire form itself is small).
pub const MAX_ANSWER_ENTRIES: usize = 1_000;

/// The type of a survey question. Constrained to the same closed set the
/// `survey_question.kind` CHECK enforces in storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestionKind {
    /// A free-text answer.
    Text,
    /// Exactly one option chosen from a list.
    SingleChoice,
    /// Zero or more options chosen from a list.
    MultipleChoice,
    /// A numeric rating on a scale (e.g. 1–5).
    Scale,
    /// A yes/no answer.
    Boolean,
}

impl QuestionKind {
    /// The stable text stored in the `survey_question.kind` column (matches the SQL CHECK).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            QuestionKind::Text => "text",
            QuestionKind::SingleChoice => "single_choice",
            QuestionKind::MultipleChoice => "multiple_choice",
            QuestionKind::Scale => "scale",
            QuestionKind::Boolean => "boolean",
        }
    }

    /// Pure lookup of a kind token; `None` for any value outside the closed set.
    #[must_use]
    fn lookup(s: &str) -> Option<Self> {
        match s {
            "text" => Some(QuestionKind::Text),
            "single_choice" => Some(QuestionKind::SingleChoice),
            "multiple_choice" => Some(QuestionKind::MultipleChoice),
            "scale" => Some(QuestionKind::Scale),
            "boolean" => Some(QuestionKind::Boolean),
            _ => None,
        }
    }

    /// Parse a question kind arriving from outside the system (request input).
    ///
    /// # Errors
    /// [`Error::Validation`] for any value outside the closed set.
    pub fn parse_input(s: &str) -> Result<Self> {
        Self::lookup(s.trim()).ok_or_else(|| {
            Error::Validation(format!(
                "kind must be one of text, single_choice, multiple_choice, scale, boolean: {s}"
            ))
        })
    }

    /// Parse a kind read back from storage. A bad value here is a schema/CHECK drift bug,
    /// not client input, so it surfaces as an internal [`Error::Storage`].
    ///
    /// # Errors
    /// [`Error::Storage`] for an unrecognised stored value.
    pub fn parse_stored(s: &str) -> Result<Self> {
        Self::lookup(s).ok_or_else(|| Error::Storage(format!("unknown question kind: {s}").into()))
    }
}

/// A validated survey creation request (title checked at the boundary).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSurvey {
    /// Title — the questionnaire heading (Portuguese civic content).
    pub title: String,
}

impl NewSurvey {
    /// Validate and normalise raw create input (trim, non-empty, length bound).
    ///
    /// # Errors
    /// [`Error::Validation`] when the title is empty or over [`MAX_TITLE_LEN`].
    pub fn validate(title: &str) -> Result<Self> {
        let title = title.trim();
        if title.is_empty() {
            return Err(Error::Validation("title must not be empty".to_string()));
        }
        if title.chars().count() > MAX_TITLE_LEN {
            return Err(Error::Validation(format!(
                "title must be at most {MAX_TITLE_LEN} characters"
            )));
        }
        Ok(Self {
            title: title.to_string(),
        })
    }
}

/// A validated question (kind + prompt + position checked at the boundary).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewQuestion {
    /// The question text shown to the respondent.
    pub prompt: String,
    /// The typed kind of answer expected.
    pub kind: QuestionKind,
    /// Zero-based position of the question on the form.
    pub position: i32,
}

impl NewQuestion {
    /// Validate and normalise a raw question (trim & bound the prompt, parse the kind,
    /// require a non-negative position).
    ///
    /// # Errors
    /// [`Error::Validation`] when the prompt is empty or over [`MAX_PROMPT_LEN`], the kind
    /// is not a recognised token, or the position is negative.
    pub fn validate(prompt: &str, kind: &str, position: i32) -> Result<Self> {
        let kind = QuestionKind::parse_input(kind)?;
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Err(Error::Validation("prompt must not be empty".to_string()));
        }
        if prompt.chars().count() > MAX_PROMPT_LEN {
            return Err(Error::Validation(format!(
                "prompt must be at most {MAX_PROMPT_LEN} characters"
            )));
        }
        if position < 0 {
            return Err(Error::Validation(
                "position must not be negative".to_string(),
            ));
        }
        Ok(Self {
            prompt: prompt.to_string(),
            kind,
            position,
        })
    }
}

/// A validated citizen response. `answers` is a JSON **object** keyed by question (the
/// shape the `survey_response.answers` jsonb column stores).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewResponse {
    /// The answers object (validated to be a non-empty, bounded JSON object).
    pub answers: serde_json::Value,
}

impl NewResponse {
    /// Validate raw answers: they must be a JSON object (not an array/scalar/null), with at
    /// least one and at most [`MAX_ANSWER_ENTRIES`] entries.
    ///
    /// # Errors
    /// [`Error::Validation`] when the answers are not a JSON object, are empty, or exceed
    /// the entry bound.
    pub fn validate(answers: serde_json::Value) -> Result<Self> {
        let obj = answers.as_object().ok_or_else(|| {
            Error::Validation("answers must be a JSON object keyed by question".to_string())
        })?;
        if obj.is_empty() {
            return Err(Error::Validation(
                "answers must contain at least one entry".to_string(),
            ));
        }
        if obj.len() > MAX_ANSWER_ENTRIES {
            return Err(Error::Validation(format!(
                "answers must contain at most {MAX_ANSWER_ENTRIES} entries"
            )));
        }
        Ok(Self { answers })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn question_kind_round_trips_through_text() {
        for k in [
            QuestionKind::Text,
            QuestionKind::SingleChoice,
            QuestionKind::MultipleChoice,
            QuestionKind::Scale,
            QuestionKind::Boolean,
        ] {
            assert_eq!(QuestionKind::parse_input(k.as_str()).unwrap(), k);
            assert_eq!(QuestionKind::parse_stored(k.as_str()).unwrap(), k);
        }
    }

    #[test]
    fn question_kind_parse_input_trims_and_rejects_unknown() {
        assert_eq!(
            QuestionKind::parse_input("  scale  ").unwrap(),
            QuestionKind::Scale
        );
        let err = QuestionKind::parse_input("ranking").unwrap_err();
        assert_eq!(err.code(), "invalid_input");
    }

    #[test]
    fn question_kind_parse_stored_rejects_unknown_as_storage() {
        let err = QuestionKind::parse_stored("bogus").unwrap_err();
        assert_eq!(err.code(), "storage_error");
    }

    #[test]
    fn new_survey_trims_and_accepts_title() {
        let s = NewSurvey::validate("  Satisfação com o transporte  ").unwrap();
        assert_eq!(s.title, "Satisfação com o transporte");
    }

    #[test]
    fn new_survey_rejects_blank_title() {
        let err = NewSurvey::validate("   ").unwrap_err();
        assert_eq!(err.code(), "invalid_input");
    }

    #[test]
    fn new_survey_rejects_overlong_title() {
        let long = "a".repeat(MAX_TITLE_LEN + 1);
        assert!(NewSurvey::validate(&long).is_err());
    }

    #[test]
    fn new_question_validates_kind_prompt_and_position() {
        let q = NewQuestion::validate("  Qual sua nota?  ", "scale", 0).unwrap();
        assert_eq!(q.prompt, "Qual sua nota?");
        assert_eq!(q.kind, QuestionKind::Scale);
        assert_eq!(q.position, 0);
    }

    #[test]
    fn new_question_rejects_bad_kind() {
        let err = NewQuestion::validate("prompt", "ranking", 0).unwrap_err();
        assert_eq!(err.code(), "invalid_input");
    }

    #[test]
    fn new_question_rejects_blank_prompt() {
        assert!(NewQuestion::validate("   ", "text", 0).is_err());
    }

    #[test]
    fn new_question_rejects_overlong_prompt() {
        let long = "a".repeat(MAX_PROMPT_LEN + 1);
        assert!(NewQuestion::validate(&long, "text", 0).is_err());
    }

    #[test]
    fn new_question_rejects_negative_position() {
        let err = NewQuestion::validate("prompt", "text", -1).unwrap_err();
        assert_eq!(err.code(), "invalid_input");
    }

    #[test]
    fn new_response_accepts_object() {
        let r = NewResponse::validate(json!({ "q1": "sim", "q2": 5 })).unwrap();
        assert_eq!(r.answers["q2"], json!(5));
    }

    #[test]
    fn new_response_rejects_non_object() {
        for v in [json!([1, 2, 3]), json!("sim"), json!(5), json!(null)] {
            let err = NewResponse::validate(v).unwrap_err();
            assert_eq!(err.code(), "invalid_input");
        }
    }

    #[test]
    fn new_response_rejects_empty_object() {
        let err = NewResponse::validate(json!({})).unwrap_err();
        assert_eq!(err.code(), "invalid_input");
    }
}

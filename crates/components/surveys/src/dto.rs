//! The crate's own request/response DTOs plus their `utoipa` schema fragment (ADR-0004:
//! `api-contract` holds only the envelope/error/pagination; each crate owns its shapes and
//! the gateway composes `/openapi.json`). Domain/storage rows are mapped to these at the HTTP
//! boundary.
//!
//! The acting citizen and org are NEVER carried in a request body — they come from the
//! verified [`dsoc_app::CallerId`] extractor (ADR-0007). Request DTOs therefore carry only
//! the payload fields.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::queries::{QuestionRow, ResponseRow, SurveyRow};

/// Public view of a survey.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SurveyDto {
    /// Survey id.
    pub id: Uuid,
    /// Owning organization.
    pub org_id: Uuid,
    /// Title — the questionnaire heading.
    pub title: String,
    /// Publication time, or `null` while the survey is a draft.
    pub published_at: Option<DateTime<Utc>>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

impl From<SurveyRow> for SurveyDto {
    fn from(row: SurveyRow) -> Self {
        Self {
            id: row.id,
            org_id: row.org_id,
            title: row.title,
            published_at: row.published_at,
            created_at: row.created_at,
        }
    }
}

/// Public view of a survey question.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct QuestionDto {
    /// Question id.
    pub id: Uuid,
    /// The survey this question belongs to.
    pub survey_id: Uuid,
    /// The question text.
    pub prompt: String,
    /// Stored kind token (`text` | `single_choice` | `multiple_choice` | `scale` | `boolean`).
    pub kind: String,
    /// Zero-based position on the form.
    pub position: i32,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

impl From<QuestionRow> for QuestionDto {
    fn from(row: QuestionRow) -> Self {
        Self {
            id: row.id,
            survey_id: row.survey_id,
            prompt: row.prompt,
            kind: row.kind,
            position: row.position,
            created_at: row.created_at,
        }
    }
}

/// Public view of a citizen response. `answers` is the parsed JSON object as stored.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ResponseDto {
    /// Response id.
    pub id: Uuid,
    /// The survey this response belongs to.
    pub survey_id: Uuid,
    /// The responding citizen.
    pub citizen_id: Uuid,
    /// The answers object keyed by question.
    #[schema(value_type = Object)]
    pub answers: serde_json::Value,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

impl From<ResponseRow> for ResponseDto {
    fn from(row: ResponseRow) -> Self {
        // The column is `jsonb NOT NULL`, so the stored text is always valid JSON; a parse
        // failure would be schema drift and is rendered as JSON null rather than panicking.
        let answers = serde_json::from_str(&row.answers).unwrap_or(serde_json::Value::Null);
        Self {
            id: row.id,
            survey_id: row.survey_id,
            citizen_id: row.citizen_id,
            answers,
            created_at: row.created_at,
        }
    }
}

/// `POST /surveys` body — create a draft survey. The owning org comes from the verified
/// [`dsoc_app::CallerId`] (ADR-0007), never the body.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct CreateSurveyRequest {
    /// Title — the questionnaire heading.
    pub title: String,
}

/// `POST /surveys/{id}/questions` body — add a typed question to a draft survey.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct AddQuestionRequest {
    /// The question text.
    pub prompt: String,
    /// The typed kind (`text` | `single_choice` | `multiple_choice` | `scale` | `boolean`).
    pub kind: String,
    /// Zero-based position on the form.
    pub position: i32,
}

/// `POST /surveys/{id}/responses` body — submit answers. The responding citizen comes from
/// the verified [`dsoc_app::CallerId`] (ADR-0007), never the body.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct SubmitResponseRequest {
    /// The answers object keyed by question.
    #[schema(value_type = Object)]
    pub answers: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn at() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-25T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn survey_dto_maps_fields_and_draft_state() {
        let row = SurveyRow {
            id: Uuid::now_v7(),
            org_id: Uuid::now_v7(),
            title: "Satisfação".to_string(),
            published_at: None,
            created_at: at(),
        };
        let dto = SurveyDto::from(row.clone());
        assert_eq!(dto.id, row.id);
        assert_eq!(dto.title, "Satisfação");
        assert!(dto.published_at.is_none());
    }

    #[test]
    fn question_dto_carries_kind_token() {
        let row = QuestionRow {
            id: Uuid::now_v7(),
            survey_id: Uuid::now_v7(),
            prompt: "Qual sua nota?".to_string(),
            kind: "scale".to_string(),
            position: 2,
            created_at: at(),
        };
        let dto = QuestionDto::from(row);
        assert_eq!(dto.kind, "scale");
        assert_eq!(dto.position, 2);
    }

    #[test]
    fn response_dto_parses_answers_json() {
        let row = ResponseRow {
            id: Uuid::now_v7(),
            survey_id: Uuid::now_v7(),
            citizen_id: Uuid::now_v7(),
            answers: r#"{"q1":"sim","q2":5}"#.to_string(),
            created_at: at(),
        };
        let dto = ResponseDto::from(row);
        assert_eq!(dto.answers, json!({ "q1": "sim", "q2": 5 }));
    }

    #[test]
    fn response_dto_renders_drifted_answers_as_null() {
        let row = ResponseRow {
            id: Uuid::now_v7(),
            survey_id: Uuid::now_v7(),
            citizen_id: Uuid::now_v7(),
            answers: "not json".to_string(),
            created_at: at(),
        };
        let dto = ResponseDto::from(row);
        assert_eq!(dto.answers, serde_json::Value::Null);
    }
}

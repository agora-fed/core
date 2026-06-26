//! The crate's own request/response DTOs plus their `utoipa` schema fragment (ADR-0004:
//! `api-contract` holds only the envelope/error/pagination; each crate owns its shapes and the
//! gateway composes `/openapi.json`). Domain types are mapped to these at the HTTP boundary.
//!
//! Note (ADR-0007): the create request carries **no** acting-citizen or org id — those come from
//! the authenticated [`dsoc_app::CallerId`], never the body. Only the consultation's own content
//! (title, window, questions) is accepted from the client.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::domain::{ConsultationView, QuestionView};

/// `POST /consultations` body: create a consultation with its ordered questions. The acting citizen
/// and org are taken from the authenticated caller, not this body.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CreateConsultationRequest {
    /// The human title of the consultation (1..=200 characters).
    pub title: String,
    /// When the response window opens.
    pub opens_at: DateTime<Utc>,
    /// When the response window closes (must be strictly after `opens_at`).
    pub closes_at: DateTime<Utc>,
    /// The ordered question prompts (1..=50, each 1..=500 characters). Position is the index.
    pub questions: Vec<String>,
}

/// A single question in a consultation response.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct QuestionDto {
    /// The question's opaque id.
    pub id: Uuid,
    /// The question text shown to participants.
    pub prompt: String,
    /// 0-based display order within the consultation.
    pub position: i32,
    /// When the question was created.
    pub created_at: DateTime<Utc>,
}

impl From<QuestionView> for QuestionDto {
    fn from(view: QuestionView) -> Self {
        Self {
            id: view.id,
            prompt: view.prompt,
            position: view.position,
            created_at: view.created_at,
        }
    }
}

/// A consultation as returned to clients, including its questions and lifecycle status.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ConsultationDto {
    /// The consultation's id (a `SpaceId` value).
    pub id: Uuid,
    /// The organization/tenant the consultation belongs to.
    pub org_id: Uuid,
    /// The human title.
    pub title: String,
    /// When the response window opens.
    pub opens_at: DateTime<Utc>,
    /// When the response window closes.
    pub closes_at: DateTime<Utc>,
    /// The lifecycle status (`"open"` or `"closed"`).
    pub status: String,
    /// When the consultation was created.
    pub created_at: DateTime<Utc>,
    /// The consultation's questions in display order.
    pub questions: Vec<QuestionDto>,
}

impl ConsultationDto {
    /// Assemble the wire DTO from a consultation view and its question views.
    #[must_use]
    pub fn from_views(view: ConsultationView, questions: Vec<QuestionView>) -> Self {
        Self {
            id: view.id.as_uuid(),
            org_id: view.org.as_uuid(),
            title: view.title,
            opens_at: view.opens_at,
            closes_at: view.closes_at,
            status: view.status.as_str().to_owned(),
            created_at: view.created_at,
            questions: questions.into_iter().map(QuestionDto::from).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::domain::ConsultationStatus;
    use dsoc_core::ids::{OrgId, SpaceId};

    fn at() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-25T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn dto_assembles_from_views_in_order() {
        let id = SpaceId::new();
        let org = OrgId::new();
        let view = ConsultationView {
            id,
            org,
            title: "Mobilidade urbana".to_owned(),
            opens_at: at(),
            closes_at: at() + chrono::Duration::days(7),
            status: ConsultationStatus::Open,
            created_at: at(),
        };
        let questions = vec![
            QuestionView {
                id: Uuid::now_v7(),
                prompt: "Q0".to_owned(),
                position: 0,
                created_at: at(),
            },
            QuestionView {
                id: Uuid::now_v7(),
                prompt: "Q1".to_owned(),
                position: 1,
                created_at: at(),
            },
        ];
        let dto = ConsultationDto::from_views(view, questions);
        assert_eq!(dto.id, id.as_uuid());
        assert_eq!(dto.org_id, org.as_uuid());
        assert_eq!(dto.status, "open");
        assert_eq!(dto.questions.len(), 2);
        assert_eq!(dto.questions[0].position, 0);
        assert_eq!(dto.questions[1].prompt, "Q1");
    }

    #[test]
    fn create_request_deserializes_without_citizen_or_org() {
        // The body shape carries only content; identity comes from CallerId (ADR-0007).
        let json = r#"{
            "title": "Consulta",
            "opens_at": "2026-07-01T00:00:00Z",
            "closes_at": "2026-07-08T00:00:00Z",
            "questions": ["Pergunta 1", "Pergunta 2"]
        }"#;
        let req: CreateConsultationRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.title, "Consulta");
        assert_eq!(req.questions.len(), 2);
        // There is no citizen_id / org_id field to trust from the body.
        assert!(!json.contains("citizen_id"));
        assert!(!json.contains("org_id"));
    }
}

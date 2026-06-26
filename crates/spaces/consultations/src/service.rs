//! The consultation service: holds the `Db` pool and the injected `Arc<dyn Clock>` (ADR-0004
//! wiring). It creates consultations with their questions in one transaction and closes them under
//! an explicit state guard. This crate emits no cross-crate events, so no `Arc<dyn EventBus>` is
//! needed here. All `sqlx` failures map onto the canonical [`dsoc_core::Error`] model.

use std::sync::Arc;

use uuid::Uuid;

use dsoc_core::ids::{OrgId, SpaceId};
use dsoc_core::{Clock, Error, Result};
use dsoc_db::Db;

use crate::domain::{self, ConsultationStatus, ConsultationView, QuestionView};
use crate::queries::{self, ConsultationRow, QuestionRow};

/// Hard cap on a consultation-listing page (unbounded reads must be bounded — PLAN.md).
const MAX_PAGE: i64 = 100;

/// Map a `sqlx` failure onto the canonical, public-safe error model (shared with [`crate::space`]).
pub(crate) fn map_sqlx(error: sqlx::Error) -> Error {
    match error {
        sqlx::Error::RowNotFound => Error::NotFound("entity not found".to_owned()),
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            Error::Conflict("entity already exists".to_owned())
        }
        other => Error::Storage(Box::new(other)),
    }
}

/// Consultation lifecycle service.
#[derive(Clone)]
pub struct ConsultationService {
    db: Db,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for ConsultationService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConsultationService")
            .finish_non_exhaustive()
    }
}

impl ConsultationService {
    /// Construct the service from its injected collaborators.
    #[must_use]
    pub fn new(db: Db, clock: Arc<dyn Clock>) -> Self {
        Self { db, clock }
    }

    /// Build a service from the shared application state.
    #[must_use]
    pub fn from_state(state: &dsoc_app::AppState) -> Self {
        Self::new(state.db.clone(), state.clock.clone())
    }

    /// Create a consultation together with its ordered questions, **all in one transaction**: a
    /// validation failure on any question or a storage failure rolls the whole thing back, so a
    /// consultation is never persisted without its questions. Returns the stored consultation and
    /// its questions.
    ///
    /// Inputs are validated at this boundary (title, window, prompts, count); the acting citizen's
    /// authorization is enforced by the HTTP layer before this method is reached.
    ///
    /// # Errors
    /// - [`Error::Validation`] when the title, window, question count, or any prompt is invalid.
    /// - [`Error::Storage`] on any persistence failure.
    pub async fn create(
        &self,
        org: OrgId,
        title: &str,
        opens_at: chrono::DateTime<chrono::Utc>,
        closes_at: chrono::DateTime<chrono::Utc>,
        prompts: &[String],
    ) -> Result<(ConsultationView, Vec<QuestionView>)> {
        if !domain::is_valid_title(title) {
            return Err(Error::Validation(
                "title must be 1..=200 characters".to_owned(),
            ));
        }
        if !domain::is_valid_window(opens_at, closes_at) {
            return Err(Error::Validation(
                "opens_at must be strictly before closes_at".to_owned(),
            ));
        }
        if !domain::is_valid_question_count(prompts.len()) {
            return Err(Error::Validation(
                "a consultation must have 1..=50 questions".to_owned(),
            ));
        }
        let trimmed_prompts: Vec<&str> = prompts.iter().map(|p| p.trim()).collect();
        if !trimmed_prompts.iter().all(|p| domain::is_valid_prompt(p)) {
            return Err(Error::Validation(
                "each question prompt must be 1..=500 characters".to_owned(),
            ));
        }

        let now = self.clock.now();
        let consultation_id = SpaceId::new();
        let status = ConsultationStatus::Open;
        let title = title.trim();

        let mut tx = self.db.begin().await.map_err(map_sqlx)?;

        let stored_id = queries::insert_consultation(
            &mut *tx,
            consultation_id.as_uuid(),
            org.as_uuid(),
            title,
            opens_at,
            closes_at,
            status.as_str(),
            now,
        )
        .await
        .map_err(map_sqlx)?;
        let id = SpaceId::from_uuid(stored_id);

        let mut questions = Vec::with_capacity(trimmed_prompts.len());
        for (index, prompt) in trimmed_prompts.iter().enumerate() {
            let position = i32::try_from(index)
                .map_err(|_| Error::Validation("too many questions to position".to_owned()))?;
            let question_id = Uuid::now_v7();
            let stored_qid = queries::insert_question(
                &mut *tx,
                question_id,
                id.as_uuid(),
                prompt,
                position,
                now,
            )
            .await
            .map_err(map_sqlx)?;
            questions.push(QuestionView {
                id: stored_qid,
                prompt: (*prompt).to_owned(),
                position,
                created_at: now,
            });
        }

        tx.commit().await.map_err(map_sqlx)?;

        let view = ConsultationView {
            id,
            org,
            title: title.to_owned(),
            opens_at,
            closes_at,
            status,
            created_at: now,
        };
        Ok((view, questions))
    }

    /// Close an open consultation. The transition is guarded by the **expected prior state**
    /// (`open`): closing an already-closed consultation is a [`Error::Conflict`], and closing a
    /// non-existent one is [`Error::NotFound`]. A redundant close is therefore safe — the state
    /// guard makes the operation effectively idempotent against a closed row.
    ///
    /// # Errors
    /// - [`Error::NotFound`] when no consultation with that id exists in the caller's org.
    /// - [`Error::Conflict`] when the consultation is already closed.
    /// - [`Error::Storage`] on any persistence failure.
    pub async fn close(&self, org: OrgId, id: SpaceId) -> Result<ConsultationView> {
        let closed = queries::close_consultation(&self.db, id.as_uuid(), org.as_uuid())
            .await
            .map_err(map_sqlx)?;
        match closed {
            Some(row) => view_from_row(row),
            None => {
                // The guarded update did not fire: disambiguate not-found from already-closed.
                let existing = queries::get_consultation(&self.db, id.as_uuid(), org.as_uuid())
                    .await
                    .map_err(map_sqlx)?;
                match existing {
                    Some(_) => Err(Error::Conflict("consultation is already closed".to_owned())),
                    None => Err(Error::NotFound("consultation not found".to_owned())),
                }
            }
        }
    }

    /// Read a consultation and its questions (public-facing). Scoped to the caller's org.
    ///
    /// # Errors
    /// [`Error::NotFound`] if no such consultation exists; [`Error::Storage`] on failure.
    pub async fn get(
        &self,
        org: OrgId,
        id: SpaceId,
    ) -> Result<(ConsultationView, Vec<QuestionView>)> {
        let row = queries::get_consultation(&self.db, id.as_uuid(), org.as_uuid())
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| Error::NotFound("consultation not found".to_owned()))?;
        let view = view_from_row(row)?;
        let questions = queries::list_questions(&self.db, id.as_uuid())
            .await
            .map_err(map_sqlx)?
            .into_iter()
            .map(question_from_row)
            .collect();
        Ok((view, questions))
    }

    /// Keyset-paginated browse of an org's consultations, ascending by id. Returns the page plus
    /// the total count for pagination metadata.
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn list(
        &self,
        org: OrgId,
        after: Option<SpaceId>,
        limit: i64,
    ) -> Result<(Vec<ConsultationView>, i64)> {
        let bounded = limit.clamp(1, MAX_PAGE);
        let rows = queries::list_consultations(
            &self.db,
            org.as_uuid(),
            after.map(|c| c.as_uuid()),
            bounded,
        )
        .await
        .map_err(map_sqlx)?;
        let total = queries::count_consultations(&self.db, org.as_uuid())
            .await
            .map_err(map_sqlx)?;
        let views = rows
            .into_iter()
            .map(view_from_row)
            .collect::<Result<Vec<_>>>()?;
        Ok((views, total))
    }
}

/// Map a consultation row to its domain view, mapping an out-of-contract status to a storage error
/// (the `CHECK` constraint should make this unreachable, but a corrupt row must not panic).
fn view_from_row(row: ConsultationRow) -> Result<ConsultationView> {
    let status = ConsultationStatus::from_db(&row.status)
        .ok_or_else(|| Error::Storage("unknown consultation status in row".into()))?;
    Ok(ConsultationView {
        id: SpaceId::from_uuid(row.id),
        org: OrgId::from_uuid(row.org_id),
        title: row.title,
        opens_at: row.opens_at,
        closes_at: row.closes_at,
        status,
        created_at: row.created_at,
    })
}

/// Map a question row to its domain view.
fn question_from_row(row: QuestionRow) -> QuestionView {
    QuestionView {
        id: row.id,
        prompt: row.prompt,
        position: row.position,
        created_at: row.created_at,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use chrono::{DateTime, Utc};

    fn at() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-25T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn map_sqlx_row_not_found_is_not_found() {
        assert_eq!(map_sqlx(sqlx::Error::RowNotFound).code(), "not_found");
    }

    #[test]
    fn map_sqlx_protocol_error_is_storage_and_hides_detail() {
        let mapped = map_sqlx(sqlx::Error::Protocol("secret".into()));
        assert_eq!(mapped.code(), "storage_error");
        assert_eq!(mapped.to_string(), "storage error");
    }

    #[test]
    fn view_from_row_maps_known_status() {
        let id = SpaceId::new();
        let org = OrgId::new();
        let row = ConsultationRow {
            id: id.as_uuid(),
            org_id: org.as_uuid(),
            title: "Plano diretor".to_owned(),
            opens_at: at(),
            closes_at: at() + chrono::Duration::days(7),
            status: "closed".to_owned(),
            created_at: at(),
        };
        let view = view_from_row(row).unwrap();
        assert_eq!(view.id, id);
        assert_eq!(view.org, org);
        assert_eq!(view.status, ConsultationStatus::Closed);
    }

    #[test]
    fn view_from_row_rejects_corrupt_status_as_storage() {
        let row = ConsultationRow {
            id: Uuid::now_v7(),
            org_id: Uuid::now_v7(),
            title: "x".to_owned(),
            opens_at: at(),
            closes_at: at(),
            status: "archived".to_owned(),
            created_at: at(),
        };
        assert_eq!(view_from_row(row).unwrap_err().code(), "storage_error");
    }

    #[test]
    fn question_from_row_maps_fields() {
        let qid = Uuid::now_v7();
        let view = question_from_row(QuestionRow {
            id: qid,
            prompt: "Pergunta?".to_owned(),
            position: 3,
            created_at: at(),
        });
        assert_eq!(view.id, qid);
        assert_eq!(view.position, 3);
        assert_eq!(view.prompt, "Pergunta?");
    }
}

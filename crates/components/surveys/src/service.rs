//! The surveys service: holds the pool and the injected [`Clock`]. It creates surveys and
//! their typed questions, publishes a survey (a guarded state transition), and collects one
//! response per citizen. All failures map onto the canonical [`dsoc_core::Error`].
//!
//! This domain emits **no** cross-crate events: `surveys.*` is not part of the frozen
//! [`dsoc_core::events::Event`] catalog and nothing consumes it, so (like `dsoc-admin`) the
//! crate keeps its state private and exposes it only through its routes. Time is read only
//! from the injected [`Clock`], never ambiently (TESTING.md).
//!
//! There is no `SurveyId` in the frozen `dsoc_core::ids`, and a survey id never crosses a
//! crate boundary (no peer consumes it), so the survey identity is a bare `uuid::Uuid` here;
//! the cross-boundary identities (`org`, `citizen`) remain the typed core newtypes.

use std::sync::Arc;

use dsoc_core::ids::{CitizenId, OrgId};
use dsoc_core::{Clock, Error, Result};
use dsoc_db::Db;
use uuid::Uuid;

use crate::domain::{NewQuestion, NewResponse, NewSurvey};
use crate::queries::{self, QuestionRow, ResponseRow, SurveyRow};

/// Default page size for listings.
pub const DEFAULT_PAGE_LIMIT: i64 = 20;
/// Hard cap on page size to keep reads bounded.
pub const MAX_PAGE_LIMIT: i64 = 100;

/// Surveys service. Cheap to construct per request from `AppState`.
#[derive(Clone)]
pub struct SurveyService {
    db: Db,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for SurveyService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SurveyService").finish_non_exhaustive()
    }
}

impl SurveyService {
    /// Build a service from an explicit pool and clock (used by tests).
    #[must_use]
    pub fn new(db: Db, clock: Arc<dyn Clock>) -> Self {
        Self { db, clock }
    }

    /// Build a service from the shared application state.
    #[must_use]
    pub fn from_state(state: &dsoc_app::AppState) -> Self {
        Self::new(state.db.clone(), state.clock.clone())
    }

    /// Create a draft survey owned by `org`. Stamped with the injected clock's time.
    ///
    /// # Errors
    /// - [`Error::Conflict`] on a unique violation.
    /// - [`Error::Validation`] on a foreign-key/check violation (e.g. unknown org).
    /// - [`Error::Storage`] on any other persistence failure.
    pub async fn create_survey(&self, org: OrgId, new: &NewSurvey) -> Result<SurveyRow> {
        queries::insert_survey(
            &self.db,
            Uuid::now_v7(),
            org.as_uuid(),
            &new.title,
            self.clock.now(),
        )
        .await
        .map_err(map_sqlx)
    }

    /// Fetch a single survey.
    ///
    /// # Errors
    /// [`Error::NotFound`] when absent; [`Error::Storage`] otherwise.
    pub async fn get_survey(&self, survey: Uuid) -> Result<SurveyRow> {
        queries::get_survey(&self.db, survey)
            .await
            .map_err(map_sqlx)
    }

    /// List an org's surveys with keyset pagination, returning the rows and the total count.
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn list_surveys(
        &self,
        org: OrgId,
        after: Option<Uuid>,
        limit: i64,
    ) -> Result<(Vec<SurveyRow>, i64)> {
        let rows = queries::list_surveys(&self.db, org.as_uuid(), after, clamp_limit(limit))
            .await
            .map_err(map_sqlx)?;
        let total = queries::count_surveys(&self.db, org.as_uuid())
            .await
            .map_err(map_sqlx)?;
        Ok((rows, total))
    }

    /// Publish a draft survey, opening it for responses. Guarded by the expected prior state:
    /// the survey must exist (else [`Error::NotFound`]) and still be a draft (a second publish
    /// is an [`Error::Conflict`], never a silent no-op). Stamped with the injected clock.
    ///
    /// # Errors
    /// - [`Error::NotFound`] when the survey does not exist.
    /// - [`Error::Conflict`] when the survey is already published.
    /// - [`Error::Storage`] on any other persistence failure.
    pub async fn publish_survey(&self, org: OrgId, survey: Uuid) -> Result<SurveyRow> {
        // Confirm the survey exists AND belongs to the caller's org (cross-tenant guard, ADR-0007):
        // a missing id is a clean 404; another org's survey is Forbidden, never mutable.
        let existing = self.get_survey(survey).await?;
        if existing.org_id != org.as_uuid() {
            return Err(Error::Forbidden(
                "survey belongs to another organization".to_string(),
            ));
        }
        match queries::publish_survey(&self.db, survey, self.clock.now())
            .await
            .map_err(map_sqlx)?
        {
            Some(row) => Ok(row),
            None => Err(Error::Conflict("survey is already published".to_string())),
        }
    }

    /// Add a typed question to a **draft** survey. The survey must exist (else
    /// [`Error::NotFound`]); a published survey is frozen, so adding a question to it is an
    /// [`Error::Conflict`] (state-transition guard).
    ///
    /// # Errors
    /// - [`Error::NotFound`] when the survey does not exist.
    /// - [`Error::Conflict`] when the survey is already published.
    /// - [`Error::Validation`] on a foreign-key/check violation.
    /// - [`Error::Storage`] on any other persistence failure.
    pub async fn add_question(
        &self,
        org: OrgId,
        survey: Uuid,
        new: &NewQuestion,
    ) -> Result<QuestionRow> {
        let survey_row = self.get_survey(survey).await?;
        if survey_row.org_id != org.as_uuid() {
            return Err(Error::Forbidden(
                "survey belongs to another organization".to_string(),
            ));
        }
        if survey_row.published_at.is_some() {
            return Err(Error::Conflict(
                "cannot add questions to a published survey".to_string(),
            ));
        }
        queries::insert_question(
            &self.db,
            Uuid::now_v7(),
            survey,
            &new.prompt,
            new.kind.as_str(),
            new.position,
            self.clock.now(),
        )
        .await
        .map_err(map_sqlx)
    }

    /// List a survey's questions with keyset pagination, returning the rows and total.
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn list_questions(
        &self,
        survey: Uuid,
        after: Option<Uuid>,
        limit: i64,
    ) -> Result<(Vec<QuestionRow>, i64)> {
        let rows = queries::list_questions(&self.db, survey, after, clamp_limit(limit))
            .await
            .map_err(map_sqlx)?;
        let total = queries::count_questions(&self.db, survey)
            .await
            .map_err(map_sqlx)?;
        Ok((rows, total))
    }

    /// Submit a citizen's response to a **published** survey. Exactly one response per citizen
    /// is kept: a re-submission is idempotent and returns the citizen's existing response (the
    /// real stored row), never a duplicate. The survey must exist and be published.
    ///
    /// # Errors
    /// - [`Error::NotFound`] when the survey does not exist.
    /// - [`Error::Conflict`] when the survey is not yet published.
    /// - [`Error::Validation`] on a foreign-key violation (e.g. unknown citizen).
    /// - [`Error::Storage`] on any other persistence failure.
    pub async fn submit_response(
        &self,
        survey: Uuid,
        citizen: CitizenId,
        new: &NewResponse,
    ) -> Result<ResponseRow> {
        let survey_row = self.get_survey(survey).await?;
        if survey_row.published_at.is_none() {
            return Err(Error::Conflict(
                "survey is not open for responses (not published)".to_string(),
            ));
        }
        let answers_json =
            serde_json::to_string(&new.answers).map_err(|e| Error::Storage(Box::new(e)))?;

        // Idempotent insert: ON CONFLICT DO NOTHING returns None for a duplicate; the existing
        // row is then re-selected so the caller always gets the real persisted id (ADR-0007).
        let inserted = queries::insert_response(
            &self.db,
            Uuid::now_v7(),
            survey,
            citizen.as_uuid(),
            &answers_json,
            self.clock.now(),
        )
        .await
        .map_err(map_sqlx)?;
        match inserted {
            Some(row) => Ok(row),
            None => queries::get_response_by_citizen(&self.db, survey, citizen.as_uuid())
                .await
                .map_err(map_sqlx),
        }
    }

    /// List a survey's responses with keyset pagination, returning the rows and total.
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn list_responses(
        &self,
        survey: Uuid,
        after: Option<Uuid>,
        limit: i64,
    ) -> Result<(Vec<ResponseRow>, i64)> {
        let rows = queries::list_responses(&self.db, survey, after, clamp_limit(limit))
            .await
            .map_err(map_sqlx)?;
        let total = queries::count_responses(&self.db, survey)
            .await
            .map_err(map_sqlx)?;
        Ok((rows, total))
    }
}

/// Clamp a requested page size into `1..=MAX_PAGE_LIMIT`.
fn clamp_limit(limit: i64) -> i64 {
    limit.clamp(1, MAX_PAGE_LIMIT)
}

/// Map a `sqlx` failure onto the canonical error model (CONTRIBUTING.md wiring conventions):
/// missing row -> `NotFound`, unique violation -> `Conflict`, foreign-key/check violation ->
/// `Validation`, everything else -> `Storage` (logged server-side, never surfaced raw).
fn map_sqlx(err: sqlx::Error) -> Error {
    match err {
        sqlx::Error::RowNotFound => Error::NotFound("survey not found".to_string()),
        sqlx::Error::Database(ref db) if db.is_unique_violation() => {
            Error::Conflict("citizen has already responded to this survey".to_string())
        }
        sqlx::Error::Database(ref db) if db.is_foreign_key_violation() => {
            Error::Validation("referenced org, survey or citizen does not exist".to_string())
        }
        sqlx::Error::Database(ref db) if db.is_check_violation() => {
            Error::Validation("input violates a constraint".to_string())
        }
        other => Error::Storage(Box::new(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_limit_bounds_page_size() {
        assert_eq!(clamp_limit(0), 1);
        assert_eq!(clamp_limit(-10), 1);
        assert_eq!(clamp_limit(20), 20);
        assert_eq!(clamp_limit(10_000), MAX_PAGE_LIMIT);
    }

    #[test]
    fn map_sqlx_row_not_found_is_not_found() {
        assert_eq!(map_sqlx(sqlx::Error::RowNotFound).code(), "not_found");
    }

    #[test]
    fn map_sqlx_other_is_storage() {
        assert_eq!(map_sqlx(sqlx::Error::PoolTimedOut).code(), "storage_error");
    }
}

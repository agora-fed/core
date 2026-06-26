//! Every database statement for surveys, as `sqlx` compile-time-checked queries (PLAN.md
//! principle 3 — no ORM, no `SELECT *`, keyset pagination on unbounded reads).
//!
//! Row shapes are returned to the service, which maps `sqlx` failures onto the canonical
//! [`dsoc_core::Error`]. Inserts use `RETURNING` so the caller always gets the real stored
//! row (never a phantom pre-generated value); the one-per-citizen response insert is
//! idempotent via `ON CONFLICT DO NOTHING` plus a re-select. Lists keyset-paginate over the
//! ascending UUIDv7 `id` (time-ordered), scoped by their parent.
//!
//! The `answers` `jsonb` column is bound/read as text via an explicit `::text::jsonb` /
//! `::text` cast so the crate needs no extra `sqlx` JSON feature; JSON shaping stays in the
//! domain/service layers.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// A persisted survey row (`survey`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurveyRow {
    /// Survey id.
    pub id: Uuid,
    /// Owning organization.
    pub org_id: Uuid,
    /// Title — the questionnaire heading.
    pub title: String,
    /// Publication time, or `None` while the survey is a draft.
    pub published_at: Option<DateTime<Utc>>,
    /// Creation time (from the injected clock).
    pub created_at: DateTime<Utc>,
}

/// A persisted question row (`survey_question`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionRow {
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

/// A persisted response row (`survey_response`). `answers` is the `jsonb` payload as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseRow {
    /// Response id.
    pub id: Uuid,
    /// The survey this response belongs to.
    pub survey_id: Uuid,
    /// The responding citizen.
    pub citizen_id: Uuid,
    /// The answers object, serialized as text.
    pub answers: String,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

/// Insert a survey (draft: `published_at` is NULL) and return the stored row.
///
/// # Errors
/// Propagates the underlying `sqlx::Error` (e.g. unknown `org_id`, or a CHECK violation).
pub async fn insert_survey(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    org_id: Uuid,
    title: &str,
    created_at: DateTime<Utc>,
) -> Result<SurveyRow, sqlx::Error> {
    let row = sqlx::query!(
        r#"INSERT INTO survey (id, org_id, title, published_at, created_at)
           VALUES ($1, $2, $3, NULL, $4)
           RETURNING id, org_id, title, published_at, created_at"#,
        id,
        org_id,
        title,
        created_at,
    )
    .fetch_one(executor)
    .await?;
    Ok(SurveyRow {
        id: row.id,
        org_id: row.org_id,
        title: row.title,
        published_at: row.published_at,
        created_at: row.created_at,
    })
}

/// Fetch a single survey by id.
///
/// # Errors
/// Propagates the underlying `sqlx::Error` (including `RowNotFound` when absent).
pub async fn get_survey(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
) -> Result<SurveyRow, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT id, org_id, title, published_at, created_at
           FROM survey
           WHERE id = $1"#,
        id,
    )
    .fetch_one(executor)
    .await?;
    Ok(SurveyRow {
        id: row.id,
        org_id: row.org_id,
        title: row.title,
        published_at: row.published_at,
        created_at: row.created_at,
    })
}

/// Mark a draft survey as published, stamping `published_at`. Guarded by the expected prior
/// state (`published_at IS NULL`) so re-publishing is a no-op that returns `None` (the
/// service maps that to a `Conflict`).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn publish_survey(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    published_at: DateTime<Utc>,
) -> Result<Option<SurveyRow>, sqlx::Error> {
    let row = sqlx::query!(
        r#"UPDATE survey
           SET published_at = $2
           WHERE id = $1 AND published_at IS NULL
           RETURNING id, org_id, title, published_at, created_at"#,
        id,
        published_at,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row.map(|row| SurveyRow {
        id: row.id,
        org_id: row.org_id,
        title: row.title,
        published_at: row.published_at,
        created_at: row.created_at,
    }))
}

/// List an org's surveys with keyset pagination over ascending `id` (UUIDv7 = time-ordered).
/// `after` is the last id seen; `None` starts from the beginning.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn list_surveys(
    executor: impl sqlx::PgExecutor<'_>,
    org_id: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<SurveyRow>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT id, org_id, title, published_at, created_at
           FROM survey
           WHERE org_id = $1 AND ($2::uuid IS NULL OR id > $2)
           ORDER BY id
           LIMIT $3"#,
        org_id,
        after,
        limit,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| SurveyRow {
            id: row.id,
            org_id: row.org_id,
            title: row.title,
            published_at: row.published_at,
            created_at: row.created_at,
        })
        .collect())
}

/// Count the surveys owned by an org (for pagination metadata).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn count_surveys(
    executor: impl sqlx::PgExecutor<'_>,
    org_id: Uuid,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT count(*) AS "count!: i64" FROM survey WHERE org_id = $1"#,
        org_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(row.count)
}

/// Insert a question and return the stored row.
///
/// # Errors
/// Propagates the underlying `sqlx::Error` (e.g. unknown `survey_id`, or a kind/position
/// CHECK violation).
pub async fn insert_question(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    survey_id: Uuid,
    prompt: &str,
    kind: &str,
    position: i32,
    created_at: DateTime<Utc>,
) -> Result<QuestionRow, sqlx::Error> {
    let row = sqlx::query!(
        r#"INSERT INTO survey_question
               (id, survey_id, prompt, kind, position, created_at)
           VALUES ($1, $2, $3, $4, $5, $6)
           RETURNING id, survey_id, prompt, kind, position, created_at"#,
        id,
        survey_id,
        prompt,
        kind,
        position,
        created_at,
    )
    .fetch_one(executor)
    .await?;
    Ok(QuestionRow {
        id: row.id,
        survey_id: row.survey_id,
        prompt: row.prompt,
        kind: row.kind,
        position: row.position,
        created_at: row.created_at,
    })
}

/// List a survey's questions with keyset pagination over ascending `id`.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn list_questions(
    executor: impl sqlx::PgExecutor<'_>,
    survey_id: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<QuestionRow>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT id, survey_id, prompt, kind, position, created_at
           FROM survey_question
           WHERE survey_id = $1 AND ($2::uuid IS NULL OR id > $2)
           ORDER BY id
           LIMIT $3"#,
        survey_id,
        after,
        limit,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| QuestionRow {
            id: row.id,
            survey_id: row.survey_id,
            prompt: row.prompt,
            kind: row.kind,
            position: row.position,
            created_at: row.created_at,
        })
        .collect())
}

/// Count the questions on a survey (for pagination metadata).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn count_questions(
    executor: impl sqlx::PgExecutor<'_>,
    survey_id: Uuid,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT count(*) AS "count!: i64" FROM survey_question WHERE survey_id = $1"#,
        survey_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(row.count)
}

/// Idempotently insert a citizen's response. On a unique `(survey_id, citizen_id)` conflict
/// the existing row wins (`DO NOTHING`) and `None` is returned; the service then re-selects
/// the stored row so the caller always gets the real persisted id (ADR-0007 idempotency).
///
/// # Errors
/// Propagates the underlying `sqlx::Error` (e.g. unknown `survey_id`/`citizen_id`).
pub async fn insert_response(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    survey_id: Uuid,
    citizen_id: Uuid,
    answers_json: &str,
    created_at: DateTime<Utc>,
) -> Result<Option<ResponseRow>, sqlx::Error> {
    let row = sqlx::query!(
        r#"INSERT INTO survey_response (id, survey_id, citizen_id, answers, created_at)
           VALUES ($1, $2, $3, $4::text::jsonb, $5)
           ON CONFLICT (survey_id, citizen_id) DO NOTHING
           RETURNING id, survey_id, citizen_id, answers::text AS "answers!", created_at"#,
        id,
        survey_id,
        citizen_id,
        answers_json,
        created_at,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row.map(|row| ResponseRow {
        id: row.id,
        survey_id: row.survey_id,
        citizen_id: row.citizen_id,
        answers: row.answers,
        created_at: row.created_at,
    }))
}

/// Fetch the existing response of a citizen to a survey (used to resolve the idempotent
/// re-select after an `ON CONFLICT DO NOTHING`).
///
/// # Errors
/// Propagates the underlying `sqlx::Error` (including `RowNotFound` when absent).
pub async fn get_response_by_citizen(
    executor: impl sqlx::PgExecutor<'_>,
    survey_id: Uuid,
    citizen_id: Uuid,
) -> Result<ResponseRow, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT id, survey_id, citizen_id, answers::text AS "answers!", created_at
           FROM survey_response
           WHERE survey_id = $1 AND citizen_id = $2"#,
        survey_id,
        citizen_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(ResponseRow {
        id: row.id,
        survey_id: row.survey_id,
        citizen_id: row.citizen_id,
        answers: row.answers,
        created_at: row.created_at,
    })
}

/// List a survey's responses with keyset pagination over ascending `id` (oldest-first).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn list_responses(
    executor: impl sqlx::PgExecutor<'_>,
    survey_id: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<ResponseRow>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT id, survey_id, citizen_id, answers::text AS "answers!", created_at
           FROM survey_response
           WHERE survey_id = $1 AND ($2::uuid IS NULL OR id > $2)
           ORDER BY id
           LIMIT $3"#,
        survey_id,
        after,
        limit,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| ResponseRow {
            id: row.id,
            survey_id: row.survey_id,
            citizen_id: row.citizen_id,
            answers: row.answers,
            created_at: row.created_at,
        })
        .collect())
}

/// Count the responses to a survey (for pagination metadata / tallies).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn count_responses(
    executor: impl sqlx::PgExecutor<'_>,
    survey_id: Uuid,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT count(*) AS "count!: i64" FROM survey_response WHERE survey_id = $1"#,
        survey_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(row.count)
}

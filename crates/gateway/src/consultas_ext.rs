//! # Consultas participativas (0.44.0, migration 0531; kinds: 0689).
//!
//! Two kinds of participatory spaces:
//!
//! - **consulta** — open federated poll (ActivityPub `Question`). Anyone can view
//!   and, in a future release, federated actors will be able to vote.
//! - **plebiscito** — agree/neutral/disagree, restricted to SSO-authenticated
//!   members of the instance (citizen must have `oidc_subject IS NOT NULL`).
//!
//! Endpoints:
//! - `GET  /consultas`                 — PUBLIC list (title, status, kind, window, question count).
//! - `GET  /consultas/{id}`            — PUBLIC detail: questions + aggregate + my answer.
//! - `POST /consultas`                 — create (gate: platform admin OR politician).
//! - `POST /consultas/{id}/responder`  — a logged-in citizen answers (only while open; plebiscito requires SSO).
//! - `POST /consultas/{id}/close`      — close (gate: platform admin OR politician).

use std::collections::{HashMap, HashSet};

use axum::extract::{Json, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, Utc};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

const DEFAULT_ORG_UUID: Uuid = uuid::uuid!("11111111-1111-1111-1111-111111111111");
const MAX_TITLE: usize = 200;
const MAX_PROMPT: usize = 500;
const MAX_QUESTIONS: usize = 20;
const LIST_LIMIT: i64 = 100;
const ANSWERS: [&str; 3] = ["concordo", "neutro", "discordo"];
const VALID_KINDS: [&str; 2] = ["consulta", "plebiscito"];

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/consultas", get(list).post(create))
        .route("/consultas/{id}", get(detail))
        .route("/consultas/{id}/responder", post(responder))
        .route("/consultas/{id}/votar", post(votar))
        .route("/consultas/{id}/close", post(close))
        .with_state(state)
}

fn caller_citizen(headers: &HeaderMap) -> Option<Uuid> {
    headers
        .get("x-dsoc-citizen-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
}

fn caller_org(headers: &HeaderMap) -> Uuid {
    headers
        .get("x-dsoc-org-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_ORG_UUID)
}

fn fail(status: StatusCode, code: &str, msg: &str) -> Response {
    (status, Json(ApiResponse::<()>::fail(code, msg))).into_response()
}

fn unauthorized() -> Response {
    fail(
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "Autenticação necessária.",
    )
}

fn storage_error() -> Response {
    fail(
        StatusCode::INTERNAL_SERVER_ERROR,
        "storage_error",
        "Erro interno.",
    )
}

/// Management gate: platform admin (owner/admin) OR an account bound to a
/// mandate (a politician). Consultations are the platform/the politician asking
/// the population. Returns Err(a ready response) when it does not pass.
async fn require_manager(db: &PgPool, headers: &HeaderMap) -> Result<Uuid, Response> {
    let Some(citizen) = caller_citizen(headers) else {
        return Err(unauthorized());
    };
    let allowed: bool = sqlx::query_scalar(
        r"SELECT
            EXISTS(SELECT 1 FROM admin_role_binding
                    WHERE org_id = $1 AND citizen_id = $2 AND role IN ('owner','admin'))
            OR EXISTS(SELECT 1 FROM mandate_identity_binding WHERE citizen_id = $2)",
    )
    .bind(caller_org(headers))
    .bind(citizen)
    .fetch_one(db)
    .await
    .unwrap_or(false);
    if allowed {
        Ok(citizen)
    } else {
        Err(fail(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Só administradores ou contas vinculadas a mandato criam consultas.",
        ))
    }
}

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, sqlx::FromRow)]
struct ConsultaSummary {
    id: Uuid,
    title: String,
    kind: String,
    status: String,
    opens_at: DateTime<Utc>,
    closes_at: DateTime<Utc>,
    question_count: i64,
}

#[derive(Debug, Serialize)]
struct Tally {
    concordo: i64,
    neutro: i64,
    discordo: i64,
    total: i64,
}

#[derive(Debug, Serialize)]
struct VoteTally {
    votes: i64,
}

#[derive(Debug, Serialize)]
struct QuestionResult {
    id: Uuid,
    prompt: String,
    position: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    tally: Option<Tally>,
    #[serde(skip_serializing_if = "Option::is_none")]
    vote_tally: Option<VoteTally>,
    /// The authenticated caller's answer (`None` = did not answer / anonymous).
    my_answer: Option<String>,
    /// The question_id the caller voted for (consulta kind only).
    #[serde(skip_serializing_if = "Option::is_none")]
    my_vote: Option<bool>,
}

#[derive(Debug, Serialize)]
struct ConsultaDetail {
    id: Uuid,
    title: String,
    kind: String,
    status: String,
    opens_at: DateTime<Utc>,
    closes_at: DateTime<Utc>,
    questions: Vec<QuestionResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ap_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateBody {
    title: String,
    #[serde(default = "default_kind")]
    kind: String,
    opens_at: DateTime<Utc>,
    closes_at: DateTime<Utc>,
    questions: Vec<String>,
}

fn default_kind() -> String {
    "plebiscito".to_owned()
}

#[derive(Debug, Deserialize)]
struct AnswerInput {
    question_id: Uuid,
    answer: String,
}

#[derive(Debug, Deserialize)]
struct ResponderBody {
    answers: Vec<AnswerInput>,
}

// ---------------------------------------------------------------------------
// GET /consultas — public list
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ListQuery {
    kind: Option<String>,
}

async fn list(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<ListQuery>,
) -> Response {
    let mut qb = sqlx::QueryBuilder::new(
        r"SELECT c.id, c.title, c.kind, c.status, c.opens_at, c.closes_at,
                 (SELECT count(*) FROM consultations_consultation_question q
                   WHERE q.consultation_id = c.id) AS question_count
            FROM consultations_consultation c
           WHERE c.org_id = ",
    );
    qb.push_bind(DEFAULT_ORG_UUID);
    if let Some(ref kind) = q.kind {
        if VALID_KINDS.contains(&kind.as_str()) {
            qb.push(" AND c.kind = ").push_bind(kind.clone());
        }
    }
    qb.push(" ORDER BY c.created_at DESC LIMIT ")
        .push_bind(LIST_LIMIT);
    let rows: Result<Vec<ConsultaSummary>, sqlx::Error> =
        qb.build_query_as().fetch_all(&state.db).await;
    match rows {
        Ok(items) => (StatusCode::OK, Json(ApiResponse::ok(items))).into_response(),
        Err(err) => {
            tracing::error!(?err, "consultas list");
            storage_error()
        }
    }
}

// ---------------------------------------------------------------------------
// GET /consultas/{id} — public detail with aggregate + my answer
// ---------------------------------------------------------------------------

async fn detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let head: Option<(String, String, String, DateTime<Utc>, DateTime<Utc>, Option<String>)> =
        match sqlx::query_as(
            r"SELECT c.title, c.kind, c.status, c.opens_at, c.closes_at,
                     f.full_path AS forum_slug
                FROM consultations_consultation c
                LEFT JOIN forum f ON f.id = c.forum_id
               WHERE c.id = $1",
        )
        .bind(id)
        .fetch_optional(&state.db)
        .await
        {
            Ok(h) => h,
            Err(err) => {
                tracing::error!(?err, "consultas detail: head");
                return storage_error();
            }
        };
    let Some((title, kind, status, opens_at, closes_at, forum_slug)) = head else {
        return fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "Consulta não encontrada.",
        );
    };

    let questions: Vec<(Uuid, String, i32)> = match sqlx::query_as(
        r"SELECT id, prompt, position FROM consultations_consultation_question
           WHERE consultation_id = $1 ORDER BY position",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await
    {
        Ok(q) => q,
        Err(err) => {
            tracing::error!(?err, "consultas detail: questions");
            return storage_error();
        }
    };
    let qids: Vec<Uuid> = questions.iter().map(|(qid, _, _)| *qid).collect();

    let is_consulta = kind == "consulta";

    // Plebiscito: aggregated per (question, answer).
    let mut by_q: HashMap<Uuid, (i64, i64, i64)> = HashMap::new();
    if !is_consulta {
        let tallies: Vec<(Uuid, String, i64)> = match sqlx::query_as(
            r"SELECT question_id, answer, count(*)
                FROM consultation_response
               WHERE question_id = ANY($1)
               GROUP BY question_id, answer",
        )
        .bind(&qids)
        .fetch_all(&state.db)
        .await
        {
            Ok(t) => t,
            Err(err) => {
                tracing::error!(?err, "consultas detail: tally");
                return storage_error();
            }
        };
        for (qid, answer, n) in tallies {
            let e = by_q.entry(qid).or_insert((0, 0, 0));
            match answer.as_str() {
                "concordo" => e.0 += n,
                "neutro" => e.1 += n,
                "discordo" => e.2 += n,
                _ => {}
            }
        }
    }

    // Consulta: vote counts per question (option).
    let mut vote_counts: HashMap<Uuid, i64> = HashMap::new();
    if is_consulta {
        let rows: Vec<(Uuid, i64)> = match sqlx::query_as(
            r"SELECT question_id, count(*) FROM consultation_vote
               WHERE question_id = ANY($1)
               GROUP BY question_id",
        )
        .bind(&qids)
        .fetch_all(&state.db)
        .await
        {
            Ok(r) => r,
            Err(err) => {
                tracing::error!(?err, "consultas detail: vote_tally");
                return storage_error();
            }
        };
        vote_counts = rows.into_iter().collect();
    }

    // My answers / votes (only with an authenticated caller).
    let mut mine: HashMap<Uuid, String> = HashMap::new();
    let mut my_voted_question: Option<Uuid> = None;
    if let Some(citizen) = caller_citizen(&headers) {
        if is_consulta {
            match sqlx::query_scalar::<_, Uuid>(
                r"SELECT question_id FROM consultation_vote
                   WHERE consultation_id = $1 AND citizen_id = $2",
            )
            .bind(id)
            .bind(citizen)
            .fetch_optional(&state.db)
            .await
            {
                Ok(v) => my_voted_question = v,
                Err(err) => tracing::error!(?err, "consultas detail: my_vote"),
            }
        } else {
            match sqlx::query_as::<_, (Uuid, String)>(
                r"SELECT question_id, answer FROM consultation_response
                   WHERE citizen_id = $1 AND question_id = ANY($2)",
            )
            .bind(citizen)
            .bind(&qids)
            .fetch_all(&state.db)
            .await
            {
                Ok(rows) => mine = rows.into_iter().collect(),
                Err(err) => tracing::error!(?err, "consultas detail: mine"),
            }
        }
    }

    let results = questions
        .into_iter()
        .map(|(qid, prompt, position)| {
            if is_consulta {
                QuestionResult {
                    id: qid,
                    prompt,
                    position,
                    tally: None,
                    vote_tally: Some(VoteTally {
                        votes: vote_counts.get(&qid).copied().unwrap_or(0),
                    }),
                    my_answer: None,
                    my_vote: my_voted_question.map(|vq| vq == qid),
                }
            } else {
                let (c, n, d) = by_q.get(&qid).copied().unwrap_or((0, 0, 0));
                QuestionResult {
                    id: qid,
                    prompt,
                    position,
                    tally: Some(Tally {
                        concordo: c,
                        neutro: n,
                        discordo: d,
                        total: c + n + d,
                    }),
                    vote_tally: None,
                    my_answer: mine.get(&qid).cloned(),
                    my_vote: None,
                }
            }
        })
        .collect();

    let ap_url = if kind == "consulta" {
        forum_slug.as_deref().map(|slug| {
            let origin = std::env::var("PUBLIC_ORIGIN")
                .unwrap_or_else(|_| "https://democracia.social.br".to_owned());
            let host = origin
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .trim_end_matches('/');
            crate::consulta_ap::consulta_object_url(host, slug, id)
        })
    } else {
        None
    };

    (
        StatusCode::OK,
        Json(ApiResponse::ok(ConsultaDetail {
            id,
            title,
            kind,
            status,
            opens_at,
            closes_at,
            questions: results,
            ap_url,
        })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// POST /consultas — create (admin OR politician)
// ---------------------------------------------------------------------------

async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateBody>,
) -> Response {
    let creator = match require_manager(&state.db, &headers).await {
        Ok(citizen) => citizen,
        Err(resp) => return resp,
    };
    let kind = body.kind.trim().to_lowercase();
    if !VALID_KINDS.contains(&kind.as_str()) {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_kind",
            "Tipo deve ser 'consulta' ou 'plebiscito'.",
        );
    }
    let title = body.title.trim();
    if title.is_empty() || title.chars().count() > MAX_TITLE {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_title",
            "Título de 1 a 200 caracteres.",
        );
    }
    if body.opens_at >= body.closes_at {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_window",
            "A abertura deve ser antes do fechamento.",
        );
    }
    let prompts: Vec<String> = body
        .questions
        .iter()
        .map(|q| q.trim().to_owned())
        .filter(|q| !q.is_empty())
        .collect();
    if prompts.is_empty() || prompts.len() > MAX_QUESTIONS {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_questions",
            "Informe de 1 a 20 perguntas.",
        );
    }
    if prompts.iter().any(|p| p.chars().count() > MAX_PROMPT) {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_prompt",
            "Pergunta longa demais (máx. 500).",
        );
    }

    let forum_id: Option<Uuid> = if kind == "consulta" {
        sqlx::query_scalar(
            "SELECT id FROM forum WHERE full_path = 'propostas' AND federated",
        )
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
    } else {
        None
    };

    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(err) => {
            tracing::error!(?err, "consultas create: begin");
            return storage_error();
        }
    };
    let cid: Uuid = match sqlx::query_scalar(
        r"INSERT INTO consultations_consultation (id, org_id, title, kind, opens_at, closes_at, status, created_at, created_by, forum_id)
          VALUES (gen_random_uuid(), $1, $2, $3, $4, $5, 'open', now(), $6, $7)
          RETURNING id",
    )
    .bind(DEFAULT_ORG_UUID)
    .bind(title)
    .bind(&kind)
    .bind(body.opens_at)
    .bind(body.closes_at)
    .bind(creator)
    .bind(forum_id)
    .fetch_one(&mut *tx)
    .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "consultas create: consultation");
            return storage_error();
        }
    };
    for (position, prompt) in prompts.iter().enumerate() {
        if let Err(err) = sqlx::query(
            r"INSERT INTO consultations_consultation_question (id, consultation_id, prompt, position, created_at)
              VALUES (gen_random_uuid(), $1, $2, $3, now())",
        )
        .bind(cid)
        .bind(prompt)
        .bind(position as i32)
        .execute(&mut *tx)
        .await
        {
            tracing::error!(?err, "consultas create: question");
            return storage_error();
        }
    }
    if let Err(err) = tx.commit().await {
        tracing::error!(?err, "consultas create: commit");
        return storage_error();
    }
    (
        StatusCode::CREATED,
        Json(ApiResponse::ok(serde_json::json!({ "id": cid }))),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// POST /consultas/{id}/responder — a citizen answers
// ---------------------------------------------------------------------------

async fn responder(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<ResponderBody>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    if body.answers.is_empty() {
        return fail(
            StatusCode::BAD_REQUEST,
            "no_answers",
            "Envie ao menos uma resposta.",
        );
    }

    // Does the consultation exist and is it open?
    let head: Option<(String, String)> =
        match sqlx::query_as("SELECT kind, status FROM consultations_consultation WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await
        {
            Ok(s) => s,
            Err(err) => {
                tracing::error!(?err, "consultas responder: status");
                return storage_error();
            }
        };
    let Some((kind, status)) = head else {
        return fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "Consulta não encontrada.",
        );
    };
    if kind == "consulta" {
        return fail(
            StatusCode::BAD_REQUEST,
            "wrong_kind",
            "Consultas usam o endpoint /votar, não /responder.",
        );
    }
    if status != "open" {
        return fail(
            StatusCode::CONFLICT,
            "closed",
            "Esta consulta está encerrada.",
        );
    }

    // Plebiscitos are restricted to SSO-authenticated members.
    if kind == "plebiscito" {
        let is_sso: bool = match sqlx::query_scalar(
            "SELECT oidc_subject IS NOT NULL FROM citizen WHERE id = $1",
        )
        .bind(citizen)
        .fetch_optional(&state.db)
        .await
        {
            Ok(Some(v)) => v,
            Ok(None) => false,
            Err(err) => {
                tracing::error!(?err, "consultas responder: sso check");
                return storage_error();
            }
        };
        if !is_sso {
            return fail(
                StatusCode::FORBIDDEN,
                "sso_required",
                "Plebiscitos são restritos a membros autenticados via SSO da Orgânica.",
            );
        }
    }

    // The set of valid questions of this consultation.
    let valid: HashSet<Uuid> = match sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM consultations_consultation_question WHERE consultation_id = $1",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await
    {
        Ok(ids) => ids.into_iter().collect(),
        Err(err) => {
            tracing::error!(?err, "consultas responder: questions");
            return storage_error();
        }
    };
    for a in &body.answers {
        if !ANSWERS.contains(&a.answer.as_str()) {
            return fail(
                StatusCode::BAD_REQUEST,
                "invalid_answer",
                "Resposta inválida.",
            );
        }
        if !valid.contains(&a.question_id) {
            return fail(
                StatusCode::BAD_REQUEST,
                "unknown_question",
                "Pergunta não pertence à consulta.",
            );
        }
    }

    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(err) => {
            tracing::error!(?err, "consultas responder: begin");
            return storage_error();
        }
    };
    for a in &body.answers {
        if let Err(err) = sqlx::query(
            r"INSERT INTO consultation_response (question_id, citizen_id, answer)
              VALUES ($1, $2, $3)
              ON CONFLICT (question_id, citizen_id)
              DO UPDATE SET answer = EXCLUDED.answer, updated_at = now()",
        )
        .bind(a.question_id)
        .bind(citizen)
        .bind(&a.answer)
        .execute(&mut *tx)
        .await
        {
            tracing::error!(?err, "consultas responder: upsert");
            return storage_error();
        }
    }
    if let Err(err) = tx.commit().await {
        tracing::error!(?err, "consultas responder: commit");
        return storage_error();
    }
    (
        StatusCode::OK,
        Json(ApiResponse::ok(
            serde_json::json!({ "saved": body.answers.len() }),
        )),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// POST /consultas/{id}/votar — single-choice vote (consulta kind)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct VotarBody {
    question_id: Uuid,
}

async fn votar(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<VotarBody>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };

    let head: Option<(String, String)> =
        match sqlx::query_as("SELECT kind, status FROM consultations_consultation WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await
        {
            Ok(s) => s,
            Err(err) => {
                tracing::error!(?err, "consultas votar: status");
                return storage_error();
            }
        };
    let Some((kind, status)) = head else {
        return fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "Consulta não encontrada.",
        );
    };
    if kind != "consulta" {
        return fail(
            StatusCode::BAD_REQUEST,
            "wrong_kind",
            "Plebiscitos usam o endpoint /responder, não /votar.",
        );
    }
    if status != "open" {
        return fail(
            StatusCode::CONFLICT,
            "closed",
            "Esta consulta está encerrada.",
        );
    }

    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM consultations_consultation_question WHERE id = $1 AND consultation_id = $2)",
    )
    .bind(body.question_id)
    .bind(id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(false);
    if !valid {
        return fail(
            StatusCode::BAD_REQUEST,
            "unknown_question",
            "Opção não pertence à consulta.",
        );
    }

    match sqlx::query(
        r"INSERT INTO consultation_vote (consultation_id, question_id, citizen_id)
          VALUES ($1, $2, $3)
          ON CONFLICT ON CONSTRAINT consultation_vote_one_per_voter
          DO UPDATE SET question_id = EXCLUDED.question_id, created_at = now()",
    )
    .bind(id)
    .bind(body.question_id)
    .bind(citizen)
    .execute(&state.db)
    .await
    {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(
                serde_json::json!({ "saved": true }),
            )),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "consultas votar: upsert");
            storage_error()
        }
    }
}

// ---------------------------------------------------------------------------
// POST /consultas/{id}/close — encerra
// ---------------------------------------------------------------------------

async fn close(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    if let Err(resp) = require_manager(&state.db, &headers).await {
        return resp;
    }
    let res = sqlx::query(
        "UPDATE consultations_consultation SET status = 'closed' WHERE id = $1 AND status = 'open'",
    )
    .bind(id)
    .execute(&state.db)
    .await;
    match res {
        Ok(r) if r.rows_affected() == 1 => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "status": "closed" }))),
        )
            .into_response(),
        Ok(_) => fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "Consulta aberta não encontrada.",
        ),
        Err(err) => {
            tracing::error!(?err, "consultas close");
            storage_error()
        }
    }
}

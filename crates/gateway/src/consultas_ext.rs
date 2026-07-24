//! # Consultas participativas (0.44.0, migration 0531) — Fase 3.3.
//!
//! O crate `dsoc-consultations` modela consulta + perguntas com janela de
//! resposta, mas: (1) toda leitura exigia login (org-scoping por `CallerId`) e
//! (2) NÃO havia como o cidadão responder. Esta superfície fecha as duas coisas
//! — leitura PÚBLICA e um mecanismo real de resposta (concordo/neutro/discordo,
//! uma por cidadão por pergunta, editável) — reusando as tabelas existentes +
//! `consultation_response` (0531). Runtime queries (sem regen do cache sqlx).
//!
//! - `GET  /consultas`                 — lista PÚBLICA (título, status, janela, nº perguntas).
//! - `GET  /consultas/{id}`            — detalhe PÚBLICO: perguntas + agregado + minha resposta.
//! - `POST /consultas`                 — cria (gate: admin de plataforma OU político).
//! - `POST /consultas/{id}/responder`  — cidadão logado responde (só com a consulta aberta).
//! - `POST /consultas/{id}/close`      — encerra (gate: admin de plataforma OU político).

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

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/consultas", get(list).post(create))
        .route("/consultas/{id}", get(detail))
        .route("/consultas/{id}/responder", post(responder))
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
    fail(StatusCode::UNAUTHORIZED, "unauthorized", "Autenticação necessária.")
}

fn storage_error() -> Response {
    fail(StatusCode::INTERNAL_SERVER_ERROR, "storage_error", "Erro interno.")
}

/// Gate de gestão: admin de plataforma (owner/admin) OU conta vinculada a
/// mandato (político). Consultas são a plataforma/o político perguntando à
/// população. Retorna Err(resposta pronta) quando não passa.
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
struct QuestionResult {
    id: Uuid,
    prompt: String,
    position: i32,
    tally: Tally,
    /// Resposta do caller autenticado (`None` = não respondeu / anônimo).
    my_answer: Option<String>,
}

#[derive(Debug, Serialize)]
struct ConsultaDetail {
    id: Uuid,
    title: String,
    status: String,
    opens_at: DateTime<Utc>,
    closes_at: DateTime<Utc>,
    questions: Vec<QuestionResult>,
}

#[derive(Debug, Deserialize)]
struct CreateBody {
    title: String,
    opens_at: DateTime<Utc>,
    closes_at: DateTime<Utc>,
    questions: Vec<String>,
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
// GET /consultas — lista pública
// ---------------------------------------------------------------------------

async fn list(State(state): State<AppState>) -> Response {
    let rows: Result<Vec<ConsultaSummary>, sqlx::Error> = sqlx::query_as(
        r"SELECT c.id, c.title, c.status, c.opens_at, c.closes_at,
                 (SELECT count(*) FROM consultations_consultation_question q
                   WHERE q.consultation_id = c.id) AS question_count
            FROM consultations_consultation c
           WHERE c.org_id = $1
           ORDER BY c.created_at DESC
           LIMIT $2",
    )
    .bind(DEFAULT_ORG_UUID)
    .bind(LIST_LIMIT)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(items) => (StatusCode::OK, Json(ApiResponse::ok(items))).into_response(),
        Err(err) => {
            tracing::error!(?err, "consultas list");
            storage_error()
        }
    }
}

// ---------------------------------------------------------------------------
// GET /consultas/{id} — detalhe público com agregado + minha resposta
// ---------------------------------------------------------------------------

async fn detail(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<Uuid>) -> Response {
    let head: Option<(String, String, DateTime<Utc>, DateTime<Utc>)> = match sqlx::query_as(
        "SELECT title, status, opens_at, closes_at FROM consultations_consultation WHERE id = $1",
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
    let Some((title, status, opens_at, closes_at)) = head else {
        return fail(StatusCode::NOT_FOUND, "not_found", "Consulta não encontrada.");
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

    // Agregado por (pergunta, resposta).
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
    let mut by_q: HashMap<Uuid, (i64, i64, i64)> = HashMap::new();
    for (qid, answer, n) in tallies {
        let e = by_q.entry(qid).or_insert((0, 0, 0));
        match answer.as_str() {
            "concordo" => e.0 += n,
            "neutro" => e.1 += n,
            "discordo" => e.2 += n,
            _ => {}
        }
    }

    // Minhas respostas (só com caller autenticado).
    let mut mine: HashMap<Uuid, String> = HashMap::new();
    if let Some(citizen) = caller_citizen(&headers) {
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

    let results = questions
        .into_iter()
        .map(|(qid, prompt, position)| {
            let (c, n, d) = by_q.get(&qid).copied().unwrap_or((0, 0, 0));
            QuestionResult {
                id: qid,
                prompt,
                position,
                tally: Tally { concordo: c, neutro: n, discordo: d, total: c + n + d },
                my_answer: mine.get(&qid).cloned(),
            }
        })
        .collect();

    (
        StatusCode::OK,
        Json(ApiResponse::ok(ConsultaDetail {
            id,
            title,
            status,
            opens_at,
            closes_at,
            questions: results,
        })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// POST /consultas — cria (admin OU político)
// ---------------------------------------------------------------------------

async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateBody>,
) -> Response {
    if let Err(resp) = require_manager(&state.db, &headers).await {
        return resp;
    }
    let title = body.title.trim();
    if title.is_empty() || title.chars().count() > MAX_TITLE {
        return fail(StatusCode::BAD_REQUEST, "invalid_title", "Título de 1 a 200 caracteres.");
    }
    if body.opens_at >= body.closes_at {
        return fail(StatusCode::BAD_REQUEST, "invalid_window", "A abertura deve ser antes do fechamento.");
    }
    let prompts: Vec<String> = body
        .questions
        .iter()
        .map(|q| q.trim().to_owned())
        .filter(|q| !q.is_empty())
        .collect();
    if prompts.is_empty() || prompts.len() > MAX_QUESTIONS {
        return fail(StatusCode::BAD_REQUEST, "invalid_questions", "Informe de 1 a 20 perguntas.");
    }
    if prompts.iter().any(|p| p.chars().count() > MAX_PROMPT) {
        return fail(StatusCode::BAD_REQUEST, "invalid_prompt", "Pergunta longa demais (máx. 500).");
    }

    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(err) => {
            tracing::error!(?err, "consultas create: begin");
            return storage_error();
        }
    };
    let cid: Uuid = match sqlx::query_scalar(
        r"INSERT INTO consultations_consultation (id, org_id, title, opens_at, closes_at, status, created_at)
          VALUES (gen_random_uuid(), $1, $2, $3, $4, 'open', now())
          RETURNING id",
    )
    .bind(DEFAULT_ORG_UUID)
    .bind(title)
    .bind(body.opens_at)
    .bind(body.closes_at)
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
    (StatusCode::CREATED, Json(ApiResponse::ok(serde_json::json!({ "id": cid })))).into_response()
}

// ---------------------------------------------------------------------------
// POST /consultas/{id}/responder — cidadão responde
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
        return fail(StatusCode::BAD_REQUEST, "no_answers", "Envie ao menos uma resposta.");
    }

    // A consulta existe e está aberta?
    let status: Option<String> =
        match sqlx::query_scalar("SELECT status FROM consultations_consultation WHERE id = $1")
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
    match status.as_deref() {
        None => return fail(StatusCode::NOT_FOUND, "not_found", "Consulta não encontrada."),
        Some("open") => {}
        Some(_) => {
            return fail(StatusCode::CONFLICT, "closed", "Esta consulta está encerrada.")
        }
    }

    // Conjunto de perguntas válidas desta consulta.
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
            return fail(StatusCode::BAD_REQUEST, "invalid_answer", "Resposta inválida.");
        }
        if !valid.contains(&a.question_id) {
            return fail(StatusCode::BAD_REQUEST, "unknown_question", "Pergunta não pertence à consulta.");
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
        Json(ApiResponse::ok(serde_json::json!({ "saved": body.answers.len() }))),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// POST /consultas/{id}/close — encerra
// ---------------------------------------------------------------------------

async fn close(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<Uuid>) -> Response {
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
        Ok(r) if r.rows_affected() == 1 => {
            (StatusCode::OK, Json(ApiResponse::ok(serde_json::json!({ "status": "closed" })))).into_response()
        }
        Ok(_) => fail(StatusCode::NOT_FOUND, "not_found", "Consulta aberta não encontrada."),
        Err(err) => {
            tracing::error!(?err, "consultas close");
            storage_error()
        }
    }
}

//! # Convite pra completar o perfil (0.49.0, migration 0534) — Fase 4 (adesão).
//!
//! Cidadão que se cadastrou mas nunca preencheu o perfil (sem `display_name` ou
//! `handle`) fica invisível na plataforma. Aqui o ADMIN (owner/admin) vê quem
//! está assim e dispara — num clique, um ou em lote — um e-mail convidando a
//! completar. `profile_nudge_sent_at` marca o envio pra não repetir sem o admin
//! mandar de novo. Nada é automático: o humano decide (LGPD/consentimento).
//!
//! - `GET  /admin/profile-nudge/overview`   — funil: total, incompletos, ainda não convidados.
//! - `GET  /admin/profile-nudge/candidates` — lista de incompletos (nome, handle, e-mail, quando).
//! - `POST /admin/profile-nudge/send`       — envia o convite pra uma seleção (1–50) e marca.

use std::time::Duration;

use axum::extract::{Json, Query, State};
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

use crate::proposal_delivery::{smtp_from_env, SmtpConfig};

const CANDIDATES_LIMIT: i64 = 500;
const MAX_BATCH: usize = 50;
/// Base pública pro link do e-mail (a página de edição de perfil).
const PROFILE_URL: &str = "https://democracia.social.br/configuracoes";

/// Predicado de "perfil incompleto": sem nome de exibição ou sem handle, e não
/// apagado. Mantido idêntico entre overview/candidates/send.
const INCOMPLETE: &str = "c.deleted_at IS NULL AND (c.display_name IS NULL OR btrim(c.display_name) = '' OR c.handle IS NULL)";

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/admin/profile-nudge/overview", get(overview))
        .route("/admin/profile-nudge/candidates", get(candidates))
        .route("/admin/profile-nudge/send", post(send))
        .with_state(state)
}

fn caller_citizen(headers: &HeaderMap) -> Option<Uuid> {
    headers
        .get("x-dsoc-citizen-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
}

fn fail(status: StatusCode, code: &str, msg: &str) -> Response {
    (status, Json(ApiResponse::<()>::fail(code, msg))).into_response()
}

fn storage_error() -> Response {
    fail(StatusCode::INTERNAL_SERVER_ERROR, "storage_error", "Erro interno.")
}

/// Gate owner/admin. Retorna Err(resposta pronta) quando não passa.
async fn require_admin(db: &PgPool, headers: &HeaderMap) -> Result<Uuid, Response> {
    let Some(citizen) = caller_citizen(headers) else {
        return Err(fail(StatusCode::UNAUTHORIZED, "unauthorized", "Autenticação necessária."));
    };
    let is_admin: bool = sqlx::query_scalar(
        r"SELECT EXISTS(
            SELECT 1 FROM admin_role_binding
             WHERE citizen_id = $1 AND role IN ('owner','admin'))",
    )
    .bind(citizen)
    .fetch_one(db)
    .await
    .unwrap_or(false);
    if is_admin {
        Ok(citizen)
    } else {
        Err(fail(StatusCode::FORBIDDEN, "forbidden", "Requer administrador."))
    }
}

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct Overview {
    total: i64,
    incomplete: i64,
    incomplete_not_nudged: i64,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct Candidate {
    citizen_id: Uuid,
    display_name: Option<String>,
    handle: Option<String>,
    email: String,
    created_at: DateTime<Utc>,
    nudged_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct CandidatesParams {
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct SendBody {
    citizen_ids: Vec<Uuid>,
}

#[derive(Debug, Serialize)]
struct SendResult {
    sent: usize,
    skipped: usize,
    failed: usize,
}

// ---------------------------------------------------------------------------
// GET /admin/profile-nudge/overview
// ---------------------------------------------------------------------------

async fn overview(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_admin(&state.db, &headers).await {
        return resp;
    }
    let row: Result<(i64, i64, i64), sqlx::Error> = sqlx::query_as(&format!(
        r"SELECT
            count(*) FILTER (WHERE c.deleted_at IS NULL) AS total,
            count(*) FILTER (WHERE {INCOMPLETE}) AS incomplete,
            count(*) FILTER (WHERE {INCOMPLETE} AND c.profile_nudge_sent_at IS NULL) AS not_nudged
          FROM citizen c
          JOIN auth_credential ac ON ac.citizen_id = c.id"
    ))
    .fetch_one(&state.db)
    .await;
    match row {
        Ok((total, incomplete, incomplete_not_nudged)) => (
            StatusCode::OK,
            Json(ApiResponse::ok(Overview { total, incomplete, incomplete_not_nudged })),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "profile_nudge overview");
            storage_error()
        }
    }
}

// ---------------------------------------------------------------------------
// GET /admin/profile-nudge/candidates
// ---------------------------------------------------------------------------

async fn candidates(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<CandidatesParams>,
) -> Response {
    if let Err(resp) = require_admin(&state.db, &headers).await {
        return resp;
    }
    let limit = params.limit.unwrap_or(CANDIDATES_LIMIT).clamp(1, CANDIDATES_LIMIT);
    let rows: Result<Vec<Candidate>, sqlx::Error> = sqlx::query_as(&format!(
        r"SELECT c.id AS citizen_id, c.display_name, c.handle, ac.email,
                 c.created_at, c.profile_nudge_sent_at AS nudged_at
            FROM citizen c
            JOIN auth_credential ac ON ac.citizen_id = c.id
           WHERE {INCOMPLETE}
           ORDER BY c.created_at DESC
           LIMIT $1"
    ))
    .bind(limit)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(items) => (StatusCode::OK, Json(ApiResponse::ok(items))).into_response(),
        Err(err) => {
            tracing::error!(?err, "profile_nudge candidates");
            storage_error()
        }
    }
}

// ---------------------------------------------------------------------------
// POST /admin/profile-nudge/send
// ---------------------------------------------------------------------------

async fn send(State(state): State<AppState>, headers: HeaderMap, Json(body): Json<SendBody>) -> Response {
    if let Err(resp) = require_admin(&state.db, &headers).await {
        return resp;
    }
    if body.citizen_ids.is_empty() || body.citizen_ids.len() > MAX_BATCH {
        return fail(StatusCode::BAD_REQUEST, "invalid_batch", "Selecione de 1 a 50 cidadãos.");
    }
    let Some(cfg) = smtp_from_env() else {
        return fail(
            StatusCode::SERVICE_UNAVAILABLE,
            "smtp_unset",
            "Envio de e-mail não está configurado no servidor.",
        );
    };

    let mut sent = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;

    for id in &body.citizen_ids {
        // Recarrega o alvo e reconfirma que AINDA está incompleto (evita mandar
        // pra quem completou entre listar e enviar). Só e-mail de quem tem credencial.
        let target: Option<(String, Option<String>)> = match sqlx::query_as(&format!(
            r"SELECT ac.email, c.display_name
                FROM citizen c JOIN auth_credential ac ON ac.citizen_id = c.id
               WHERE c.id = $1 AND {INCOMPLETE}"
        ))
        .bind(id)
        .fetch_optional(&state.db)
        .await
        {
            Ok(t) => t,
            Err(err) => {
                tracing::error!(?err, "profile_nudge send: load");
                failed += 1;
                continue;
            }
        };
        let Some((email, display_name)) = target else {
            skipped += 1; // não existe, apagado, ou já completou
            continue;
        };

        let first = display_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("Olá");
        let subject = "Complete seu perfil na DemocraciaBR";
        let text = format!(
            "{first},\n\n\
             Você criou sua conta na DemocraciaBR, mas seu perfil ainda está incompleto. \
             Um perfil completo (nome e @usuário) faz você ser reconhecido(a) quando apoia \
             uma proposta, participa de um debate ou responde a uma consulta.\n\n\
             Leva menos de um minuto — é só completar aqui:\n{PROFILE_URL}\n\n\
             Se não quiser mais receber estes lembretes, é só responder este e-mail.\n\n\
             — DemocraciaBR"
        );

        match send_mail(&cfg, &email, subject, &text).await {
            Ok(()) => {
                let _ = sqlx::query("UPDATE citizen SET profile_nudge_sent_at = now() WHERE id = $1")
                    .bind(id)
                    .execute(&state.db)
                    .await;
                sent += 1;
            }
            Err(err) => {
                tracing::error!(?err, "profile_nudge send: smtp");
                failed += 1;
            }
        }
    }

    (StatusCode::OK, Json(ApiResponse::ok(SendResult { sent, skipped, failed }))).into_response()
}

/// Envia um e-mail de texto simples pelo relay SMTP soberano (mesmo transporte
/// do /contato e do password-reset).
async fn send_mail(
    cfg: &SmtpConfig,
    to: &str,
    subject: &str,
    body: &str,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use lettre::message::header::ContentType;
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::transport::smtp::AsyncSmtpTransport;
    use lettre::{AsyncTransport, Message, Tokio1Executor};

    let mut builder = if cfg.port == 465 {
        AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.host)?
    } else {
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.host)?
    };
    builder = builder.port(cfg.port).timeout(Some(Duration::from_secs(5)));
    if let (Some(u), Some(p)) = (cfg.user.as_ref(), cfg.pass.as_ref()) {
        builder = builder.credentials(Credentials::new(u.clone(), p.clone()));
    }
    let mailer = builder.build();

    let email = Message::builder()
        .from(cfg.from.parse()?)
        .to(to.parse()?)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_owned())?;
    mailer.send(email).await?;
    Ok(())
}

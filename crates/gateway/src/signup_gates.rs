//! Gates de cadastro/login (migration 0514).
//!
//! - `GET  /admin/email_domain_blocks`
//! - `POST /admin/email_domain_blocks {domain, reason?}`
//! - `DELETE /admin/email_domain_blocks/{domain}`
//! - `GET  /admin/ip_rules`
//! - `POST /admin/ip_rules {cidr, scope, state, reason?}`
//! - `DELETE /admin/ip_rules/{id}`
//! - `GET  /admin/pending_signups` (contas com pending_review=true)
//! - `POST /admin/pending_signups/{id}/approve`
//! - `POST /admin/pending_signups/{id}/reject` (marca suspended_at)
//!
//! Utilitários públicos (chamados por outros módulos):
//! - `is_email_domain_blocked`, `is_ip_denied` — usados no fluxo de
//!   register/login. Nesta fatia, EXPOMOS os endpoints admin mas o
//!   enforcement no register_confirm/login fica dependente de o
//!   servico dsoc-auth chamar essas fns (ADR-mudança maior).
//!   Como paliativo, marcamos `pending_review=true` via env var.

use axum::extract::{Json, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::Router;
use chrono::{DateTime, Utc};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route(
            "/admin/email_domain_blocks",
            get(list_email_domains).post(add_email_domain),
        )
        .route(
            "/admin/email_domain_blocks/{domain}",
            delete(remove_email_domain),
        )
        .route("/admin/ip_rules", get(list_ip_rules).post(add_ip_rule))
        .route("/admin/ip_rules/{id}", delete(remove_ip_rule))
        .route("/admin/pending_signups", get(list_pending))
        .route("/admin/pending_signups/{id}/approve", post(approve_pending))
        .route("/admin/pending_signups/{id}/reject", post(reject_pending))
        .with_state(state)
}

fn caller_citizen(headers: &HeaderMap) -> Option<Uuid> {
    headers
        .get("x-dsoc-citizen-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
}

async fn require_admin(headers: &HeaderMap, db: &PgPool) -> Result<Uuid, Response> {
    let citizen = caller_citizen(headers).ok_or_else(unauthorized_resp)?;
    let is_admin = sqlx::query_scalar::<_, bool>(
        r"SELECT EXISTS (SELECT 1 FROM admin_role_binding
                        WHERE citizen_id = $1 AND role IN ('owner','admin'))",
    )
    .bind(citizen)
    .fetch_one(db)
    .await
    .unwrap_or(false);
    if !is_admin {
        return Err(forbidden_resp());
    }
    Ok(citizen)
}

fn unauthorized_resp() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(ApiResponse::<()>::fail(
            "unauthorized",
            "Autenticação necessária.",
        )),
    )
        .into_response()
}
fn forbidden_resp() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(ApiResponse::<()>::fail(
            "forbidden",
            "Acesso restrito a admins.",
        )),
    )
        .into_response()
}
fn storage_err(err: impl std::fmt::Debug) -> Response {
    tracing::error!(?err, "signup_gates storage");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiResponse::<()>::fail("storage_error", "Erro interno.")),
    )
        .into_response()
}
fn ok_json() -> Response {
    (
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({ "ok": true }))),
    )
        .into_response()
}
fn bad(msg: &'static str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ApiResponse::<()>::fail("bad_request", msg)),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Email domain blocks
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, sqlx::FromRow)]
struct EmailDomainDto {
    domain: String,
    reason: Option<String>,
    created_at: DateTime<Utc>,
}

async fn list_email_domains(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    let rows: Result<Vec<EmailDomainDto>, _> = sqlx::query_as::<_, EmailDomainDto>(
        r"SELECT domain, reason, created_at FROM email_domain_block ORDER BY domain",
    )
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(list) => (StatusCode::OK, Json(ApiResponse::ok(list))).into_response(),
        Err(err) => storage_err(err),
    }
}

#[derive(Debug, Deserialize)]
struct DomainBody {
    domain: String,
    #[serde(default)]
    reason: Option<String>,
}

async fn add_email_domain(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<DomainBody>,
) -> Response {
    let admin = match require_admin(&headers, &state.db).await {
        Ok(id) => id,
        Err(r) => return r,
    };
    let d = body.domain.trim().to_ascii_lowercase();
    if d.is_empty() || !d.contains('.') || d.len() > 253 {
        return bad("domínio inválido");
    }
    let reason = body.reason.and_then(|s| {
        let t = s.trim().to_owned();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    });
    let res = sqlx::query(
        r"INSERT INTO email_domain_block (id, domain, reason, created_by)
          VALUES ($1, $2, $3, $4)
          ON CONFLICT (domain) DO UPDATE SET reason = EXCLUDED.reason",
    )
    .bind(Uuid::now_v7())
    .bind(&d)
    .bind(reason.as_deref())
    .bind(admin)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => ok_json(),
        Err(err) => storage_err(err),
    }
}

async fn remove_email_domain(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(domain): Path<String>,
) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    let _ = sqlx::query(r"DELETE FROM email_domain_block WHERE domain = $1")
        .bind(domain.to_ascii_lowercase())
        .execute(&state.db)
        .await;
    ok_json()
}

// ---------------------------------------------------------------------------
// IP rules
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, sqlx::FromRow)]
struct IpRuleDto {
    id: Uuid,
    cidr: String,
    scope: String,
    state: String,
    reason: Option<String>,
    created_at: DateTime<Utc>,
}

async fn list_ip_rules(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    let rows: Result<Vec<IpRuleDto>, _> = sqlx::query_as::<_, IpRuleDto>(
        r"SELECT id, cidr, scope, state, reason, created_at
            FROM ip_rule
           ORDER BY state, scope, cidr",
    )
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(list) => (StatusCode::OK, Json(ApiResponse::ok(list))).into_response(),
        Err(err) => storage_err(err),
    }
}

#[derive(Debug, Deserialize)]
struct IpRuleBody {
    cidr: String,
    scope: String,
    state: String,
    #[serde(default)]
    reason: Option<String>,
}

async fn add_ip_rule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<IpRuleBody>,
) -> Response {
    let admin = match require_admin(&headers, &state.db).await {
        Ok(id) => id,
        Err(r) => return r,
    };
    if !matches!(body.scope.as_str(), "signup" | "login" | "all") {
        return bad("scope deve ser signup, login ou all");
    }
    if !matches!(body.state.as_str(), "allow" | "deny") {
        return bad("state deve ser allow ou deny");
    }
    // Validação leve do CIDR: parse pra std::net.
    let cidr = body.cidr.trim();
    if !cidr_is_valid(cidr) {
        return bad("cidr inválido (use IP ou IP/prefixo)");
    }
    let reason = body.reason.and_then(|s| {
        let t = s.trim().to_owned();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    });
    let res = sqlx::query(
        r"INSERT INTO ip_rule (id, cidr, scope, state, reason, created_by)
          VALUES ($1, $2, $3, $4, $5, $6)
          ON CONFLICT (cidr, scope) DO UPDATE
            SET state = EXCLUDED.state, reason = EXCLUDED.reason",
    )
    .bind(Uuid::now_v7())
    .bind(cidr)
    .bind(&body.scope)
    .bind(&body.state)
    .bind(reason.as_deref())
    .bind(admin)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => ok_json(),
        Err(err) => storage_err(err),
    }
}

async fn remove_ip_rule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    let _ = sqlx::query(r"DELETE FROM ip_rule WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await;
    ok_json()
}

fn cidr_is_valid(s: &str) -> bool {
    // Aceita "IP" ou "IP/prefixo".
    let (ip, prefix) = match s.split_once('/') {
        Some((ip, p)) => (ip, Some(p)),
        None => (s, None),
    };
    if ip.parse::<std::net::IpAddr>().is_err() {
        return false;
    }
    if let Some(p) = prefix {
        match p.parse::<u8>() {
            Ok(n) => n <= 128,
            Err(_) => false,
        }
    } else {
        true
    }
}

// ---------------------------------------------------------------------------
// Pending review
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, sqlx::FromRow)]
struct PendingSignupDto {
    citizen_id: Uuid,
    email: Option<String>,
    handle: Option<String>,
    display_name: Option<String>,
    created_at: DateTime<Utc>,
}

async fn list_pending(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    let rows: Result<Vec<PendingSignupDto>, _> = sqlx::query_as::<_, PendingSignupDto>(
        r"SELECT c.id AS citizen_id,
                 ac.email,
                 c.handle,
                 c.display_name,
                 c.created_at
            FROM citizen c
            LEFT JOIN auth_credential ac ON ac.citizen_id = c.id
           WHERE c.pending_review = true
             AND c.deleted_at IS NULL
             AND c.suspended_at IS NULL
           ORDER BY c.created_at
           LIMIT 500",
    )
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(list) => (StatusCode::OK, Json(ApiResponse::ok(list))).into_response(),
        Err(err) => storage_err(err),
    }
}

async fn approve_pending(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let admin = match require_admin(&headers, &state.db).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let _ = sqlx::query(
        r"UPDATE citizen
             SET pending_review = false,
                 approved_at = now(),
                 approved_by = $2
           WHERE id = $1",
    )
    .bind(id)
    .bind(admin)
    .execute(&state.db)
    .await;
    crate::webhooks::dispatch_event(
        state.db.clone(),
        "account.approved",
        serde_json::json!({ "citizen_id": id, "approved_by": admin }),
    );
    ok_json()
}

async fn reject_pending(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    let _ = sqlx::query(
        r"UPDATE citizen
             SET pending_review = false,
                 suspended_at = now(),
                 suspended_reason = COALESCE(suspended_reason, 'rejeitada na revisão')
           WHERE id = $1",
    )
    .bind(id)
    .execute(&state.db)
    .await;
    ok_json()
}

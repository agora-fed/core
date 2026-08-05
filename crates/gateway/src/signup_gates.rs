//! Gates de cadastro/login (migration 0514).
//!
//! - `GET  /admin/email_domain_blocks`
//! - `POST /admin/email_domain_blocks {domain, reason?}`
//! - `DELETE /admin/email_domain_blocks/{domain}`
//! - `GET  /admin/ip_rules`
//! - `POST /admin/ip_rules {cidr, scope, state, reason?}`
//! - `DELETE /admin/ip_rules/{id}`
//! - `GET  /admin/pending_signups` (accounts with pending_review=true)
//! - `POST /admin/pending_signups/{id}/approve`
//! - `POST /admin/pending_signups/{id}/reject` (marca suspended_at)
//!
//! **Enforcement (0.28.2)** — [`gates_middleware`], aplicado no router
//! `/api/v1` router (the tables belong to this module; dsoc-auth does not
//! know them — no cross-crate coupling):
//! - `POST …/auth/register[/politician]`: refuses an e-mail whose domain is in
//!   `email_domain_block` e IP negado por `ip_rule` (escopo signup/all);
//! - `POST …/auth/login`: recusa IP negado por `ip_rule` (escopo login/all).
//!
//! `citizen.pending_review` is enforced INSIDE dsoc-auth (its own table):
//! `GATEWAY_SIGNUP_REQUIRES_REVIEW=true` makes confirm create the account
//! pending, and login refuses it until approval in /admin/revisoes.

use std::net::IpAddr;

use axum::body::Body;
use axum::extract::{Json, Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
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
    // Light CIDR validation: parse into std::net.
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

// ---------------------------------------------------------------------------
// Enforcement — middleware nas rotas de register/login (0.28.2)
// ---------------------------------------------------------------------------

/// Cap of the body buffered by the middleware on gated routes. Real
/// register/login payloads are ~200 bytes; 32 KiB leaves room without opening a DoS.
const GATE_BODY_LIMIT: usize = 32 * 1024;

/// Middleware aplicado ao router `/api/v1`: intercepta register/login e
/// aplica as regras administradas em /admin/email-domains e /admin/ip-rules.
/// Falha ABERTA em erro de DB (log + segue) — indisponibilidade de storage
/// cannot bring down signup/login entirely; the rule applies again on the
/// next healthy request.
pub async fn gates_middleware(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let path = req.uri().path();
    let is_register =
        path.ends_with("/auth/register") || path.ends_with("/auth/register/politician");
    let is_login = path.ends_with("/auth/login");
    if !is_register && !is_login {
        return next.run(req).await;
    }

    let ip = client_ip(req.headers());
    let scope = if is_register { "signup" } else { "login" };
    if let Some(ip) = ip {
        if ip_denied(&state.db, ip, scope).await {
            return gate_denied();
        }
    }

    if is_register {
        // The e-mail domain lives in the body — buffer it (with a cap), inspect it
        // and hand the same bytes back to the handler's Json extractor.
        let (parts, body) = req.into_parts();
        let bytes = match axum::body::to_bytes(body, GATE_BODY_LIMIT).await {
            Ok(b) => b,
            Err(_) => {
                return (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    Json(ApiResponse::<()>::fail(
                        "payload_too_large",
                        "Requisição grande demais.",
                    )),
                )
                    .into_response();
            }
        };
        let email = serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()
            .and_then(|v| v.get("email").and_then(|e| e.as_str()).map(str::to_owned));
        if let Some(email) = email {
            if email_domain_blocked(&state.db, &email).await {
                return gate_denied();
            }
        }
        let req = Request::from_parts(parts, Body::from(bytes));
        return next.run(req).await;
    }

    next.run(req).await
}

/// The same X-Forwarded-For read as the rest of the gateway (behind Caddy).
/// No header ⇒ no IP check — an identical posture to the rate limits.
fn client_ip(headers: &HeaderMap) -> Option<IpAddr> {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .and_then(|s| s.trim().parse().ok())
}

/// A single response for any gate — deliberately not saying WHICH rule
/// blocked (never teach the blocked party how to work around it).
fn gate_denied() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(ApiResponse::<()>::fail(
            "gate_denied",
            "Cadastro/login não disponível para esta origem. \
             Fale conosco pelo formulário de contato se acredita ser um engano.",
        )),
    )
        .into_response()
}

/// Is the e-mail's domain in `email_domain_block`? Fail-open on error.
async fn email_domain_blocked(db: &PgPool, email: &str) -> bool {
    let Some(domain) = email
        .rsplit_once('@')
        .map(|(_, d)| d.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|d| !d.is_empty())
    else {
        return false; // e-mail sem @ — deixa o handler rejeitar com 400 próprio
    };
    match sqlx::query_scalar::<_, bool>(
        r"SELECT EXISTS (SELECT 1 FROM email_domain_block WHERE domain = $1)",
    )
    .bind(&domain)
    .fetch_one(db)
    .await
    {
        Ok(blocked) => blocked,
        Err(err) => {
            tracing::error!(
                ?err,
                "signup gate: email_domain_block lookup failed (fail-open)"
            );
            false
        }
    }
}

/// Is the IP denied by the scope's `ip_rule`s? Fail-open on a DB error.
async fn ip_denied(db: &PgPool, ip: IpAddr, scope: &str) -> bool {
    let rules: Vec<(String, String)> =
        match sqlx::query_as(r"SELECT cidr, state FROM ip_rule WHERE scope = $1 OR scope = 'all'")
            .bind(scope)
            .fetch_all(db)
            .await
        {
            Ok(rows) => rows,
            Err(err) => {
                tracing::error!(?err, "signup gate: ip_rule lookup failed (fail-open)");
                return false;
            }
        };
    ip_denied_by_rules(ip, &rules)
}

/// 0514 semantics: a matching deny denies; if ANY allow exists in the
/// scope, the pool becomes an allowlist (an IP outside every allow is denied);
/// allowlist vazia = todos passam.
fn ip_denied_by_rules(ip: IpAddr, rules: &[(String, String)]) -> bool {
    let mut has_allow = false;
    let mut allowed = false;
    for (cidr, state) in rules {
        let hit = cidr_match(ip, cidr);
        match state.as_str() {
            "deny" if hit => return true,
            "allow" => {
                has_allow = true;
                allowed = allowed || hit;
            }
            _ => {}
        }
    }
    has_allow && !allowed
}

/// Does `ip` belong to `cidr` ("a.b.c.d", "a.b.c.d/nn", "xx::/nn")? Different
/// families never match; a missing prefix = a single host; an invalid prefix
/// never matches (a malformed rule must never deny by accident).
fn cidr_match(ip: IpAddr, cidr: &str) -> bool {
    let (base, prefix) = match cidr.split_once('/') {
        Some((b, p)) => (b, p.parse::<u32>().ok()),
        None => (cidr, None),
    };
    let Ok(base) = base.trim().parse::<IpAddr>() else {
        return false;
    };
    match (ip, base) {
        (IpAddr::V4(ip), IpAddr::V4(base)) => {
            let bits = prefix.unwrap_or(32);
            if bits > 32 {
                return false;
            }
            let mask = if bits == 0 {
                0
            } else {
                u32::MAX << (32 - bits)
            };
            (u32::from(ip) & mask) == (u32::from(base) & mask)
        }
        (IpAddr::V6(ip), IpAddr::V6(base)) => {
            let bits = prefix.unwrap_or(128);
            if bits > 128 {
                return false;
            }
            let mask = if bits == 0 {
                0
            } else {
                u128::MAX << (128 - bits)
            };
            (u128::from(ip) & mask) == (u128::from(base) & mask)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn cidr_match_v4_v6_and_malformed() {
        assert!(cidr_match(ip("192.168.1.7"), "192.168.1.0/24"));
        assert!(!cidr_match(ip("192.168.2.7"), "192.168.1.0/24"));
        assert!(cidr_match(ip("203.0.113.5"), "203.0.113.5"));
        assert!(cidr_match(ip("2804:710:d0:9::a000"), "2804:710:d0::/48"));
        assert!(!cidr_match(ip("2804:711::1"), "2804:710:d0::/48"));
        // A crossed family and a malformed rule never match.
        assert!(!cidr_match(ip("192.168.1.7"), "2804:710::/32"));
        assert!(!cidr_match(ip("192.168.1.7"), "not-a-cidr"));
        assert!(!cidr_match(ip("192.168.1.7"), "192.168.1.0/99"));
    }

    #[test]
    fn deny_rule_wins_and_matches() {
        let rules = vec![("10.0.0.0/8".to_owned(), "deny".to_owned())];
        assert!(ip_denied_by_rules(ip("10.1.2.3"), &rules));
        assert!(!ip_denied_by_rules(ip("11.1.2.3"), &rules));
    }

    #[test]
    fn allow_pool_becomes_allowlist() {
        let rules = vec![
            ("192.168.0.0/16".to_owned(), "allow".to_owned()),
            ("10.0.0.0/8".to_owned(), "deny".to_owned()),
        ];
        // Dentro do allow: passa. Fora de todo allow: negado. Deny sempre nega.
        assert!(!ip_denied_by_rules(ip("192.168.5.5"), &rules));
        assert!(ip_denied_by_rules(ip("172.16.0.1"), &rules));
        assert!(ip_denied_by_rules(ip("10.0.0.1"), &rules));
        // With no rule at all: everyone passes.
        assert!(!ip_denied_by_rules(ip("8.8.8.8"), &[]));
    }
}

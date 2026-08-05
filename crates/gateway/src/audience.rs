//! # Contact base / audience (0.35.0).
//!
//! "I wanted a good base first." This module is the single base of
//! pessoas em potencial:
//!
//! Public:
//! - `POST /audience/subscribe`  — capture from the site ("get updates"):
//!   e-mail + optional name/state, LGPD consent, honeypot + per-IP rate limit
//!   (the same posture as the contact form).
//! - `GET  /audience/unsubscribe?token=…` — one-click opt-out, no
//!   login; returns a minimal HTML page. It never deletes the row (the opt-out
//!   registrado protege contra re-import acidental).
//!
//! Admin:
//! - `GET    /admin/audience/stats`      — totals + a breakdown per segment.
//! - `GET    /admin/audience`            — filterable listing.
//! - `POST   /admin/audience/import`     — import an existing list with a
//!   declared legal basis; never downgrades consent nor resurrects an opt-out.
//! - `GET    /admin/audience/export.csv` — a base em CSV.
//! - `DELETE /admin/audience/{id}`       — permanent removal (LGPD).

use axum::extract::{Json, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::Router;
use chrono::{DateTime, Utc};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

const MAX_IMPORT: usize = 2_000;
const RATE_WINDOW: Duration = Duration::from_secs(3_600);
const DEFAULT_RATE_MAX_PER_HOUR: usize = 10;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/audience/subscribe", post(subscribe))
        .route("/audience/unsubscribe", get(unsubscribe))
        .route("/admin/audience/stats", get(stats))
        .route("/admin/audience", get(list))
        .route("/admin/audience/import", post(import))
        .route("/admin/audience/export.csv", get(export_csv))
        .route("/admin/audience/{id}", delete(remove))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Public — capture and opt-out
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SubscribeBody {
    email: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    uf: Option<String>,
    /// Honeypot — a human never sees the field; filled = bot, fake success.
    #[serde(default)]
    website: String,
}

fn valid_email(e: &str) -> bool {
    e.len() >= 5 && e.len() <= 254 && e.contains('@') && !e.contains(char::is_whitespace)
}

fn caller_ip(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

static RATE: LazyLock<Mutex<HashMap<String, Vec<Instant>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn rate_allow(ip: Option<&str>) -> bool {
    let Some(ip) = ip else { return true };
    let max = std::env::var("AUDIENCE_RATE_MAX_PER_HOUR")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_RATE_MAX_PER_HOUR);
    let now = Instant::now();
    let mut map = RATE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    map.retain(|_, hits| {
        hits.retain(|t| now.duration_since(*t) < RATE_WINDOW);
        !hits.is_empty()
    });
    let hits = map.entry(ip.to_owned()).or_default();
    if hits.len() >= max {
        return false;
    }
    hits.push(now);
    true
}

/// `POST /audience/subscribe` — the site's form. Re-subscribing after an
/// opt-out is intentional: the person ASKED again (renewed consent).
async fn subscribe(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SubscribeBody>,
) -> Response {
    if !body.website.trim().is_empty() {
        // A bot in the honeypot: fake success, no effect.
        return (StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response();
    }
    let email = body.email.trim().to_lowercase();
    if !valid_email(&email) {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_email",
            "Informe um e-mail válido.",
        );
    }
    if !rate_allow(caller_ip(&headers).as_deref()) {
        return fail(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "Muitas inscrições deste endereço. Tente novamente em uma hora.",
        );
    }
    let name = body
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= 120)
        .map(str::to_owned);
    let uf = body
        .uf
        .as_deref()
        .map(|s| s.trim().to_uppercase())
        .filter(|s| s.len() == 2 && s.chars().all(|c| c.is_ascii_alphabetic()));
    let res = sqlx::query(
        r"INSERT INTO audience_contact
              (email, name, uf, segment, source, legal_basis, consented_at)
          VALUES ($1, $2, $3, 'cidadao', 'site_form', 'consent', now())
          ON CONFLICT (lower(email)) DO UPDATE SET
              name            = COALESCE(EXCLUDED.name, audience_contact.name),
              uf              = COALESCE(EXCLUDED.uf, audience_contact.uf),
              legal_basis     = 'consent',
              consented_at    = now(),
              unsubscribed_at = NULL,
              updated_at      = now()",
    )
    .bind(&email)
    .bind(&name)
    .bind(&uf)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => (StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response(),
        Err(err) => storage_err(err),
    }
}

#[derive(Debug, Deserialize)]
struct UnsubQuery {
    token: String,
}

/// `GET /audience/unsubscribe?token=…` — a minimal page, no login. An unknown
/// token receives the SAME page (it never leaks whether the e-mail exists).
async fn unsubscribe(State(state): State<AppState>, Query(q): Query<UnsubQuery>) -> Response {
    let token = q.token.trim();
    if !token.is_empty() && token.len() <= 64 {
        let _ = sqlx::query(
            r"UPDATE audience_contact
                 SET unsubscribed_at = COALESCE(unsubscribed_at, now()),
                     updated_at = now()
               WHERE unsubscribe_token = $1",
        )
        .bind(token)
        .execute(&state.db)
        .await;
    }
    Html(
        r#"<!DOCTYPE html><html lang="pt-BR"><head><meta charset="utf-8">
<title>Descadastro — DemocraciaBR</title></head>
<body style="font-family:sans-serif;max-width:480px;margin:80px auto;text-align:center;color:#0f172a;">
<h1 style="color:#15803d;">Pronto.</h1>
<p>Você não receberá mais e-mails da DemocraciaBR neste endereço.</p>
<p>Mudou de ideia? É só se inscrever de novo em
<a href="https://democracia.social.br" style="color:#15803d;">democracia.social.br</a>.</p>
</body></html>"#,
    )
    .into_response()
}

// ---------------------------------------------------------------------------
// Admin
// ---------------------------------------------------------------------------

/// Org-scoped admin gate — delegates to the single implementation in
/// [`crate::authz_ext::require_org_admin`] (issue #8). This module used to carry
/// its own copy that omitted `org_id`, so an owner of ANY org passed it.
async fn require_admin(headers: &HeaderMap, db: &PgPool) -> Result<Uuid, Response> {
    crate::authz_ext::require_org_admin(db, headers)
        .await
        .map(|a| a.citizen)
}

#[derive(Debug, Serialize)]
struct SegmentCountDto {
    segment: String,
    active: i64,
}

#[derive(Debug, Serialize)]
struct StatsDto {
    total: i64,
    active: i64,
    unsubscribed: i64,
    from_site: i64,
    imported: i64,
    segments: Vec<SegmentCountDto>,
}

async fn stats(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_admin(&headers, &state.db).await {
        return resp;
    }
    let totals: Result<(i64, i64, i64, i64, i64), _> = sqlx::query_as(
        r"SELECT count(*),
                 count(*) FILTER (WHERE unsubscribed_at IS NULL),
                 count(*) FILTER (WHERE unsubscribed_at IS NOT NULL),
                 count(*) FILTER (WHERE source = 'site_form'),
                 count(*) FILTER (WHERE source LIKE 'import:%')
            FROM audience_contact",
    )
    .fetch_one(&state.db)
    .await;
    let (total, active, unsubscribed, from_site, imported) = match totals {
        Ok(t) => t,
        Err(err) => return storage_err(err),
    };
    let segments: Vec<(String, i64)> = sqlx::query_as(
        r"SELECT segment, count(*)
            FROM audience_contact
           WHERE unsubscribed_at IS NULL
           GROUP BY segment
           ORDER BY count(*) DESC",
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    (
        StatusCode::OK,
        Json(ApiResponse::ok(StatsDto {
            total,
            active,
            unsubscribed,
            from_site,
            imported,
            segments: segments
                .into_iter()
                .map(|(segment, active)| SegmentCountDto { segment, active })
                .collect(),
        })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    segment: Option<String>,
    /// active | unsubscribed | all (default active)
    status: Option<String>,
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Debug, Serialize)]
struct ContactDto {
    id: Uuid,
    email: String,
    name: Option<String>,
    uf: Option<String>,
    segment: String,
    source: String,
    legal_basis: String,
    unsubscribed: bool,
    created_at: DateTime<Utc>,
}

const LIST_SQL: &str = r"
    SELECT id, email, name, uf, segment, source, legal_basis,
           unsubscribed_at IS NOT NULL, created_at
      FROM audience_contact
     WHERE ($1::text IS NULL OR segment = $1)
       AND ($2::text = 'all'
            OR ($2 = 'active' AND unsubscribed_at IS NULL)
            OR ($2 = 'unsubscribed' AND unsubscribed_at IS NOT NULL))
       AND ($3::text IS NULL
            OR email ILIKE '%' || $3 || '%'
            OR COALESCE(name,'') ILIKE '%' || $3 || '%')
     ORDER BY created_at DESC
     LIMIT $4 OFFSET $5";

type ContactRow = (
    Uuid,
    String,
    Option<String>,
    Option<String>,
    String,
    String,
    String,
    bool,
    DateTime<Utc>,
);

async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Response {
    if let Err(resp) = require_admin(&headers, &state.db).await {
        return resp;
    }
    let status = match q.status.as_deref().unwrap_or("active") {
        s @ ("active" | "unsubscribed" | "all") => s.to_owned(),
        _ => {
            return fail(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "status deve ser active, unsubscribed ou all",
            );
        }
    };
    let rows: Result<Vec<ContactRow>, _> = sqlx::query_as(LIST_SQL)
        .bind(&q.segment)
        .bind(&status)
        .bind(q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()))
        .bind(q.limit.unwrap_or(100).clamp(1, 500))
        .bind(q.offset.unwrap_or(0).max(0))
        .fetch_all(&state.db)
        .await;
    match rows {
        Ok(rows) => {
            let dtos: Vec<ContactDto> = rows
                .into_iter()
                .map(
                    |(
                        id,
                        email,
                        name,
                        uf,
                        segment,
                        source,
                        legal_basis,
                        unsubscribed,
                        created_at,
                    )| {
                        ContactDto {
                            id,
                            email,
                            name,
                            uf,
                            segment,
                            source,
                            legal_basis,
                            unsubscribed,
                            created_at,
                        }
                    },
                )
                .collect();
            (StatusCode::OK, Json(ApiResponse::ok(dtos))).into_response()
        }
        Err(err) => storage_err(err),
    }
}

#[derive(Debug, Deserialize)]
struct ImportContact {
    email: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    uf: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ImportBody {
    /// Short slug identifying the list (becomes `source = 'import:<slug>'`).
    source_slug: String,
    /// LGPD: 'consent' only when the list was collected WITH consent for
    /// this purpose; otherwise 'legitimate_interest' + notes explaining its origin.
    legal_basis: String,
    #[serde(default)]
    segment: Option<String>,
    #[serde(default)]
    notes: Option<String>,
    contacts: Vec<ImportContact>,
}

#[derive(Debug, Serialize)]
struct ImportResultDto {
    received: usize,
    upserted: usize,
    invalid: usize,
}

/// `POST /admin/audience/import`. Import NUNCA rebaixa consent existente
/// nor clears an opt-out — whoever unsubscribed stays unsubscribed even
/// even when it appears in the imported list.
async fn import(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ImportBody>,
) -> Response {
    if let Err(resp) = require_admin(&headers, &state.db).await {
        return resp;
    }
    if !matches!(body.legal_basis.as_str(), "consent" | "legitimate_interest") {
        return fail(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "legal_basis deve ser consent ou legitimate_interest",
        );
    }
    let slug: String = body
        .source_slug
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(40)
        .collect();
    if slug.is_empty() {
        return fail(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "source_slug obrigatório (a-z, 0-9, hífen).",
        );
    }
    if body.contacts.is_empty() || body.contacts.len() > MAX_IMPORT {
        return fail(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "contacts deve ter entre 1 e 2000 itens por chamada.",
        );
    }
    let segment = body
        .segment
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("cidadao")
        .to_owned();
    let source = format!("import:{slug}");
    let mut upserted = 0usize;
    let mut invalid = 0usize;
    for c in &body.contacts {
        let email = c.email.trim().to_lowercase();
        if !valid_email(&email) {
            invalid += 1;
            continue;
        }
        let name = c
            .name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty() && s.len() <= 120);
        let uf =
            c.uf.as_deref()
                .map(|s| s.trim().to_uppercase())
                .filter(|s| s.len() == 2);
        let res = sqlx::query(
            r"INSERT INTO audience_contact
                  (email, name, uf, segment, source, legal_basis,
                   consented_at, notes)
              VALUES ($1, $2, $3, $4, $5, $6,
                      CASE WHEN $6 = 'consent' THEN now() END, $7)
              ON CONFLICT (lower(email)) DO UPDATE SET
                  name       = COALESCE(audience_contact.name, EXCLUDED.name),
                  uf         = COALESCE(audience_contact.uf, EXCLUDED.uf),
                  updated_at = now()",
        )
        .bind(&email)
        .bind(name)
        .bind(&uf)
        .bind(&segment)
        .bind(&source)
        .bind(&body.legal_basis)
        .bind(body.notes.as_deref())
        .execute(&state.db)
        .await;
        match res {
            Ok(_) => upserted += 1,
            Err(err) => {
                tracing::warn!(?err, "audience import: upsert falhou");
                invalid += 1;
            }
        }
    }
    (
        StatusCode::OK,
        Json(ApiResponse::ok(ImportResultDto {
            received: body.contacts.len(),
            upserted,
            invalid,
        })),
    )
        .into_response()
}

/// `GET /admin/audience/export.csv` — a base inteira em CSV.
async fn export_csv(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_admin(&headers, &state.db).await {
        return resp;
    }
    type ExportRow = (
        String,
        Option<String>,
        Option<String>,
        String,
        String,
        String,
        Option<DateTime<Utc>>,
        DateTime<Utc>,
    );
    let rows: Vec<ExportRow> = match sqlx::query_as(
        r"SELECT email, name, uf, segment, source, legal_basis,
                 unsubscribed_at, created_at
            FROM audience_contact ORDER BY created_at",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(r) => r,
        Err(err) => return storage_err(err),
    };
    let esc = |s: &str| {
        if s.contains(',') || s.contains('"') || s.contains('\n') {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_owned()
        }
    };
    let mut csv = String::from("email,name,uf,segment,source,legal_basis,status,created_at\n");
    for (email, name, uf, segment, source, legal_basis, unsub, created) in rows {
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{}\n",
            esc(&email),
            esc(name.as_deref().unwrap_or("")),
            esc(uf.as_deref().unwrap_or("")),
            esc(&segment),
            esc(&source),
            esc(&legal_basis),
            if unsub.is_some() {
                "descadastrado"
            } else {
                "ativo"
            },
            created.to_rfc3339(),
        ));
    }
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"base-contatos.csv\"".to_owned(),
            ),
        ],
        csv,
    )
        .into_response()
}

/// `DELETE /admin/audience/{id}` — permanent removal (an LGPD request).
async fn remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    if let Err(resp) = require_admin(&headers, &state.db).await {
        return resp;
    }
    match sqlx::query("DELETE FROM audience_contact WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
    {
        Ok(r) if r.rows_affected() == 0 => fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "Contato não encontrado.",
        ),
        Ok(_) => (StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response(),
        Err(err) => storage_err(err),
    }
}

fn fail(status: StatusCode, code: &str, msg: &str) -> Response {
    (status, Json(ApiResponse::<()>::fail(code, msg))).into_response()
}

fn storage_err(err: impl std::fmt::Debug) -> Response {
    tracing::error!(?err, "audience: storage error");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiResponse::<()>::fail("storage_error", "Erro interno.")),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_validation() {
        assert!(valid_email("ana@exemplo.br"));
        assert!(!valid_email("a@b"));
        assert!(!valid_email("sem-arroba.br"));
        assert!(!valid_email("com espaco@x.br"));
    }
}

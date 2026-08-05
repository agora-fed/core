//! Webhooks servidor-wide (migration 0515).
//!
//! Endpoints admin:
//! - `GET  /admin/webhooks`
//! - `POST /admin/webhooks {url, events[]}`
//! - `PATCH /admin/webhooks/{id} {enabled?}`
//! - `DELETE /admin/webhooks/{id}`
//!
//! Dispatch: call `dispatch_event(db, event, payload)` from any module
//! when something relevant happens. Fire-and-forget: a delivery failure becomes a
//! log + last_status, but never blocks the caller. Retry is left to the next
//! iteration (a worker loop).

use axum::extract::{Json, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch};
use axum::Router;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Utc};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use hmac::{Hmac, Mac};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use sqlx::PgPool;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/admin/webhooks", get(list_admin).post(create))
        .route("/admin/webhooks/{id}", patch(update).delete(delete_webhook))
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
    tracing::error!(?err, "webhooks storage");
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
// DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, sqlx::FromRow)]
struct WebhookDto {
    id: Uuid,
    url: String,
    events: Vec<String>,
    enabled: bool,
    last_status: Option<i32>,
    last_delivery_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct WebhookWithSecretDto {
    id: Uuid,
    url: String,
    events: Vec<String>,
    enabled: bool,
    /// Shown only once, at creation.
    secret: String,
    created_at: DateTime<Utc>,
}

async fn list_admin(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    let rows: Result<Vec<WebhookDto>, _> = sqlx::query_as::<_, WebhookDto>(
        r"SELECT id, url, events, enabled, last_status, last_delivery_at, created_at
            FROM webhook
           ORDER BY created_at DESC",
    )
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(list) => (StatusCode::OK, Json(ApiResponse::ok(list))).into_response(),
        Err(err) => storage_err(err),
    }
}

#[derive(Debug, Deserialize)]
struct CreateBody {
    url: String,
    events: Vec<String>,
}

const VALID_EVENTS: &[&str] = &[
    "report.created",
    "account.approved",
    "account.suspended",
    "account.silenced",
];

fn generate_secret() -> String {
    let mut buf = [0u8; 32];
    OsRng.fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateBody>,
) -> Response {
    let admin = match require_admin(&headers, &state.db).await {
        Ok(id) => id,
        Err(r) => return r,
    };
    let url = body.url.trim();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return bad("url deve começar com http(s)://");
    }
    let events: Vec<String> = body
        .events
        .into_iter()
        .filter(|e| VALID_EVENTS.contains(&e.as_str()))
        .collect();
    if events.is_empty() {
        return bad("selecione ao menos 1 evento válido");
    }
    let secret = generate_secret();
    let id = Uuid::now_v7();
    let res = sqlx::query(
        r"INSERT INTO webhook (id, url, events, secret, created_by)
          VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(url)
    .bind(&events)
    .bind(&secret)
    .bind(admin)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(WebhookWithSecretDto {
                id,
                url: url.to_owned(),
                events,
                enabled: true,
                secret,
                created_at: Utc::now(),
            })),
        )
            .into_response(),
        Err(err) => storage_err(err),
    }
}

#[derive(Debug, Deserialize)]
struct UpdateBody {
    enabled: Option<bool>,
}

async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateBody>,
) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    if let Some(e) = body.enabled {
        let _ = sqlx::query(r"UPDATE webhook SET enabled = $2 WHERE id = $1")
            .bind(id)
            .bind(e)
            .execute(&state.db)
            .await;
    }
    ok_json()
}

async fn delete_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    let _ = sqlx::query(r"DELETE FROM webhook WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await;
    ok_json()
}

// ---------------------------------------------------------------------------
// Dispatch: call from any module when something relevant happens.
// ---------------------------------------------------------------------------

/// Fire `event` at every enabled webhook subscribing to that type.
/// Fire-and-forget: each delivery runs in its own task, writes the
/// result to `last_status`, and never blocks the caller.
pub fn dispatch_event(db: PgPool, event: &'static str, payload: serde_json::Value) {
    tokio::spawn(async move {
        let hooks: Vec<(Uuid, String, String)> = sqlx::query_as::<_, (Uuid, String, String)>(
            r"SELECT id, url, secret
                FROM webhook
               WHERE enabled = true AND $1 = ANY(events)",
        )
        .bind(event)
        .fetch_all(&db)
        .await
        .unwrap_or_default();
        if hooks.is_empty() {
            return;
        }
        let body = serde_json::json!({ "event": event, "data": payload });
        let body_bytes = match serde_json::to_vec(&body) {
            Ok(b) => b,
            Err(_) => return,
        };
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .user_agent("DemocraciaBR-Webhook/1.0")
            .build();
        let Ok(client) = client else { return };
        for (id, url, secret) in hooks {
            let sig = {
                let mut mac = match HmacSha256::new_from_slice(secret.as_bytes()) {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                mac.update(&body_bytes);
                let bytes = mac.finalize().into_bytes();
                URL_SAFE_NO_PAD.encode(bytes)
            };
            let client = client.clone();
            let db = db.clone();
            let body_bytes = body_bytes.clone();
            tokio::spawn(async move {
                let resp = client
                    .post(&url)
                    .header("content-type", "application/json")
                    .header("x-democraciabr-event", event)
                    .header("x-democraciabr-signature", format!("sha256={sig}"))
                    .body(body_bytes)
                    .send()
                    .await;
                let status = resp
                    .as_ref()
                    .map(|r| r.status().as_u16() as i32)
                    .unwrap_or(0);
                let _ = sqlx::query(
                    r"UPDATE webhook
                         SET last_status = $2, last_delivery_at = now()
                       WHERE id = $1",
                )
                .bind(id)
                .bind(status)
                .execute(&db)
                .await;
                if let Err(err) = resp {
                    tracing::warn!(?err, ?url, "webhook delivery failed");
                }
            });
        }
    });
}

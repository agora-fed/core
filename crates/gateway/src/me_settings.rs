//! # Settings endpoints for the authenticated citizen (0.20.0-settings).
//!
//! Two small surfaces the `/configuracoes` page needs beyond the pre-existing
//! profile + sessions endpoints:
//!
//! * `GET  /api/v1/me/authorized-apps`               — Mastodon OAuth apps that
//!   currently hold a live bearer token for this citizen.
//! * `POST /api/v1/me/authorized-apps/{app_id}/revoke` — revoke every live
//!   token this citizen has issued to that app.
//! * `POST /api/v1/me/change-password`               — verify current password,
//!   set new one (Argon2id), kill every OTHER session so a leaked cookie can't
//!   linger.
//!
//! Every handler is guarded by cookie/bearer auth via the `x-dsoc-citizen-id`
//! header — same pattern as `admin_ext.rs`.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use dsoc_app::AppState;
use dsoc_api_contract::ApiResponse;
use dsoc_auth::credential::{hash_password, verify_password};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/me/authorized-apps", get(list_authorized_apps))
        .route(
            "/me/authorized-apps/{app_id}/revoke",
            post(revoke_authorized_app),
        )
        .route("/me/change-password", post(change_password))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Guards / helpers (mirror the shape of admin_ext.rs)
// ---------------------------------------------------------------------------

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(ApiResponse::<()>::fail("http_401", "Autenticação necessária.")),
    )
        .into_response()
}

fn server_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiResponse::<()>::fail("http_500", "Erro interno.")),
    )
        .into_response()
}

fn bad_request(msg: &'static str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ApiResponse::<()>::fail("http_400", msg)),
    )
        .into_response()
}

fn caller_citizen(headers: &HeaderMap) -> Option<Uuid> {
    headers
        .get("x-dsoc-citizen-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| Uuid::parse_str(s).ok())
}

fn caller_session(headers: &HeaderMap) -> Option<Uuid> {
    // Reads `dsoc_session=<uuid>` from the Cookie header. Used by
    // change_password so we can keep the current browser signed in while
    // killing every OTHER session for this citizen.
    let cookie = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    for kv in cookie.split(';') {
        let kv = kv.trim();
        if let Some(rest) = kv.strip_prefix("dsoc_session=") {
            return Uuid::parse_str(rest).ok();
        }
    }
    None
}

// ---------------------------------------------------------------------------
// GET /me/authorized-apps
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct AuthorizedAppDto {
    application_id: Uuid,
    name: String,
    website: Option<String>,
    scopes: String,
    /// Number of currently live (non-revoked, non-expired) tokens this citizen
    /// has for this app. Usually 1, but a re-auth from the same app can pile
    /// up a couple.
    token_count: i64,
    /// The earliest still-valid token — approximates "connected since".
    first_authorized_at: DateTime<Utc>,
    /// The latest token expiry — approximates "expires".
    last_expires_at: DateTime<Utc>,
}

async fn list_authorized_apps(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let rows: Result<Vec<(Uuid, String, Option<String>, String, i64, DateTime<Utc>, DateTime<Utc>)>, _> =
        sqlx::query_as(
            r"SELECT app.id, app.name, app.website, MAX(tok.scopes),
                     COUNT(tok.id) AS token_count,
                     MIN(tok.created_at) AS first_authorized_at,
                     MAX(tok.expires_at) AS last_expires_at
                FROM oauth_access_token tok
                JOIN oauth_application app ON app.id = tok.application_id
               WHERE tok.citizen_id = $1
                 AND tok.revoked_at IS NULL
                 AND tok.expires_at > now()
               GROUP BY app.id, app.name, app.website
               ORDER BY MAX(tok.created_at) DESC",
        )
        .bind(citizen)
        .fetch_all(&state.db)
        .await;
    let rows = match rows {
        Ok(rows) => rows,
        Err(err) => {
            tracing::error!(?err, "list_authorized_apps sql");
            return server_error();
        }
    };
    let list: Vec<AuthorizedAppDto> = rows
        .into_iter()
        .map(|(id, name, website, scopes, count, first, last)| AuthorizedAppDto {
            application_id: id,
            name,
            website,
            scopes,
            token_count: count,
            first_authorized_at: first,
            last_expires_at: last,
        })
        .collect();
    (StatusCode::OK, Json(ApiResponse::ok(list))).into_response()
}

// ---------------------------------------------------------------------------
// POST /me/authorized-apps/{app_id}/revoke
// ---------------------------------------------------------------------------

async fn revoke_authorized_app(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(app_id): Path<Uuid>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let res = sqlx::query(
        r"UPDATE oauth_access_token
             SET revoked_at = now()
           WHERE application_id = $1
             AND citizen_id = $2
             AND revoked_at IS NULL",
    )
    .bind(app_id)
    .bind(citizen)
    .execute(&state.db)
    .await;
    match res {
        Ok(r) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({
                "revoked": r.rows_affected(),
            }))),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "revoke_authorized_app sql");
            server_error()
        }
    }
}

// ---------------------------------------------------------------------------
// POST /me/change-password
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ChangePasswordReq {
    current_password: String,
    new_password: String,
}

async fn change_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ChangePasswordReq>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    if body.new_password.len() < 8 {
        return bad_request("A nova senha deve ter ao menos 8 caracteres.");
    }
    if body.new_password == body.current_password {
        return bad_request("A nova senha precisa ser diferente da atual.");
    }
    // 1. Fetch current password hash.
    let row: Result<Option<(String,)>, _> = sqlx::query_as(
        r"SELECT password_hash FROM auth_credential WHERE citizen_id = $1",
    )
    .bind(citizen)
    .fetch_optional(&state.db)
    .await;
    let stored = match row {
        Ok(Some((h,))) => h,
        Ok(None) => {
            // Citizen authenticated via cookie but has no credential row — must be
            // an OIDC-linked account. Password change is not available then.
            return bad_request(
                "Sua conta usa login externo — a senha não pode ser trocada aqui.",
            );
        }
        Err(err) => {
            tracing::error!(?err, "change_password fetch");
            return server_error();
        }
    };
    if !verify_password(&body.current_password, &stored) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(ApiResponse::<()>::fail("http_401", "Senha atual incorreta.")),
        )
            .into_response();
    }
    let new_hash = match hash_password(&body.new_password) {
        Ok(h) => h,
        Err(_) => return bad_request("A nova senha é inválida."),
    };
    // 2. Update hash + kill every OTHER session in a single tx.
    let current_session = caller_session(&headers);
    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(err) => {
            tracing::error!(?err, "change_password tx begin");
            return server_error();
        }
    };
    if let Err(err) = sqlx::query(
        r"UPDATE auth_credential SET password_hash = $2 WHERE citizen_id = $1",
    )
    .bind(citizen)
    .bind(&new_hash)
    .execute(&mut *tx)
    .await
    {
        tracing::error!(?err, "change_password update hash");
        return server_error();
    }
    // Preserve the current browser's session; kill everything else.
    let kill_others = if let Some(sid) = current_session {
        sqlx::query(
            r"DELETE FROM auth_session WHERE citizen_id = $1 AND id <> $2",
        )
        .bind(citizen)
        .bind(sid)
        .execute(&mut *tx)
        .await
    } else {
        sqlx::query(r"DELETE FROM auth_session WHERE citizen_id = $1")
            .bind(citizen)
            .execute(&mut *tx)
            .await
    };
    if let Err(err) = kill_others {
        tracing::error!(?err, "change_password kill others");
        return server_error();
    }
    // Also revoke every existing OAuth bearer — a stolen password could have
    // been used to authorize an app already.
    if let Err(err) = sqlx::query(
        r"UPDATE oauth_access_token SET revoked_at = now()
           WHERE citizen_id = $1 AND revoked_at IS NULL",
    )
    .bind(citizen)
    .execute(&mut *tx)
    .await
    {
        tracing::error!(?err, "change_password revoke tokens");
        return server_error();
    }
    if let Err(err) = tx.commit().await {
        tracing::error!(?err, "change_password commit");
        return server_error();
    }
    (
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({
            "ok": true,
        }))),
    )
        .into_response()
}

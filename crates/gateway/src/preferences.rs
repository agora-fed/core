//! Personal preferences + server rules (migration 0511).
//!
//! - `GET  /api/v1/me/preferences` — bloco { email_prefs, default_visibility, default_sensitive }.
//! - `PATCH /api/v1/me/preferences` — mesclagem parcial.
//! - `GET  /api/v1/server/rules` — public, ordered.
//! - `GET  /api/v1/admin/rules` — admin.
//! - `POST /api/v1/admin/rules {text, ordinal?}` — create.
//! - `PATCH /api/v1/admin/rules/{id}` — edita.
//! - `DELETE /api/v1/admin/rules/{id}` — apaga.

use axum::extract::{Json, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch};
use axum::Router;
use chrono::{DateTime, Utc};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/me/preferences", get(get_prefs).patch(patch_prefs))
        .route("/server/rules", get(public_rules))
        .route(
            "/admin/rules",
            get(admin_list_rules).post(admin_create_rule),
        )
        .route(
            "/admin/rules/{id}",
            patch(admin_update_rule).delete(admin_delete_rule),
        )
        // 0.26.20: editable Terms + CW presets.
        .route("/server/terms", get(public_terms))
        .route(
            "/admin/server/terms",
            get(admin_get_terms).patch(admin_patch_terms),
        )
        .route("/server/cw_presets", get(public_cw_presets))
        .route(
            "/admin/cw_presets",
            get(admin_list_cw_presets).post(admin_create_cw_preset),
        )
        .route("/admin/cw_presets/{id}", delete(admin_delete_cw_preset))
        .with_state(state)
}

async fn public_terms(State(state): State<AppState>) -> Response {
    let row: Option<(String, DateTime<Utc>)> = sqlx::query_as::<_, (String, DateTime<Utc>)>(
        r"SELECT body, updated_at FROM server_terms WHERE id = 1",
    )
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);
    let body = row
        .map(|(b, u)| serde_json::json!({ "body": b, "updated_at": u }))
        .unwrap_or_else(|| serde_json::json!({ "body": null, "updated_at": null }));
    (StatusCode::OK, Json(ApiResponse::ok(body))).into_response()
}

async fn admin_get_terms(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    public_terms(State(state)).await
}

#[derive(Debug, Deserialize)]
struct TermsPatch {
    body: String,
}

async fn admin_patch_terms(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TermsPatch>,
) -> Response {
    let admin = match require_admin(&headers, &state.db).await {
        Ok(id) => id,
        Err(r) => return r,
    };
    let txt = body.body.trim();
    if txt.is_empty() || txt.len() > 100_000 {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::fail(
                "bad_request",
                "corpo obrigatório (até 100 KB)",
            )),
        )
            .into_response();
    }
    let res = sqlx::query(
        r"INSERT INTO server_terms (id, body, updated_by) VALUES (1, $1, $2)
          ON CONFLICT (id) DO UPDATE SET body = EXCLUDED.body,
                                         updated_at = now(),
                                         updated_by = EXCLUDED.updated_by",
    )
    .bind(txt)
    .bind(admin)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => ok_json(),
        Err(err) => storage_err(err),
    }
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct CwPresetDto {
    id: Uuid,
    phrase: String,
    spoiler_text: Option<String>,
    created_at: DateTime<Utc>,
}

async fn public_cw_presets(State(state): State<AppState>) -> Response {
    let rows: Result<Vec<CwPresetDto>, _> = sqlx::query_as::<_, CwPresetDto>(
        r"SELECT id, phrase, spoiler_text, created_at FROM cw_preset ORDER BY phrase",
    )
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(list) => (StatusCode::OK, Json(ApiResponse::ok(list))).into_response(),
        Err(err) => storage_err(err),
    }
}

async fn admin_list_cw_presets(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    public_cw_presets(State(state)).await
}

#[derive(Debug, Deserialize)]
struct CwPresetCreate {
    phrase: String,
    #[serde(default)]
    spoiler_text: Option<String>,
}

async fn admin_create_cw_preset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CwPresetCreate>,
) -> Response {
    let admin = match require_admin(&headers, &state.db).await {
        Ok(id) => id,
        Err(r) => return r,
    };
    let phrase = body.phrase.trim();
    if phrase.is_empty() || phrase.len() > 200 {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::fail(
                "bad_request",
                "phrase 1..200 chars",
            )),
        )
            .into_response();
    }
    let spoiler = body.spoiler_text.and_then(|s| {
        let t = s.trim().to_owned();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    });
    let res: Result<CwPresetDto, _> = sqlx::query_as::<_, CwPresetDto>(
        r"INSERT INTO cw_preset (id, phrase, spoiler_text, created_by)
          VALUES ($1, $2, $3, $4)
          ON CONFLICT (phrase) DO UPDATE SET spoiler_text = EXCLUDED.spoiler_text
          RETURNING id, phrase, spoiler_text, created_at",
    )
    .bind(Uuid::now_v7())
    .bind(phrase)
    .bind(spoiler.as_deref())
    .bind(admin)
    .fetch_one(&state.db)
    .await;
    match res {
        Ok(dto) => (StatusCode::OK, Json(ApiResponse::ok(dto))).into_response(),
        Err(err) => storage_err(err),
    }
}

async fn admin_delete_cw_preset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    let _ = sqlx::query(r"DELETE FROM cw_preset WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await;
    ok_json()
}

fn caller_citizen(headers: &HeaderMap) -> Option<Uuid> {
    headers
        .get("x-dsoc-citizen-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
}

/// Org-scoped admin gate — delegates to the single implementation in
/// [`crate::authz_ext::require_org_admin`] (issue #8). This module used to carry
/// its own copy that omitted `org_id`, so an owner of ANY org passed it.
async fn require_admin(headers: &HeaderMap, db: &PgPool) -> Result<Uuid, Response> {
    crate::authz_ext::require_org_admin(db, headers)
        .await
        .map(|a| a.citizen)
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
fn storage_err(err: impl std::fmt::Debug) -> Response {
    tracing::error!(?err, "preferences storage");
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

// ---------------------------------------------------------------------------
// Preferences
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct PrefsDto {
    email_prefs: Value,
    default_visibility: String,
    default_sensitive: bool,
    auto_federate_threshold: bool,
}

#[derive(Debug, Deserialize)]
struct PatchPrefs {
    email_prefs: Option<Value>,
    default_visibility: Option<String>,
    default_sensitive: Option<bool>,
    auto_federate_threshold: Option<bool>,
}

async fn get_prefs(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized_resp();
    };
    let row: Result<Option<(Value, String, bool, bool)>, _> =
        sqlx::query_as::<_, (Value, String, bool, bool)>(
            r"SELECT email_prefs, default_visibility, default_sensitive,
                     auto_federate_threshold
                FROM citizen WHERE id = $1",
        )
        .bind(citizen)
        .fetch_optional(&state.db)
        .await;
    match row {
        Ok(Some((email_prefs, default_visibility, default_sensitive, auto_federate_threshold))) => {
            (
                StatusCode::OK,
                Json(ApiResponse::ok(PrefsDto {
                    email_prefs,
                    default_visibility,
                    default_sensitive,
                    auto_federate_threshold,
                })),
            )
                .into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::fail("not_found", "conta não encontrada")),
        )
            .into_response(),
        Err(err) => storage_err(err),
    }
}

async fn patch_prefs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PatchPrefs>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized_resp();
    };
    if let Some(v) = body.default_visibility.as_deref() {
        if !matches!(v, "public" | "unlisted" | "followers" | "direct") {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::<()>::fail(
                    "bad_request",
                    "default_visibility inválida",
                )),
            )
                .into_response();
        }
        let _ = sqlx::query(r"UPDATE citizen SET default_visibility = $2 WHERE id = $1")
            .bind(citizen)
            .bind(v)
            .execute(&state.db)
            .await;
    }
    if let Some(b) = body.default_sensitive {
        let _ = sqlx::query(r"UPDATE citizen SET default_sensitive = $2 WHERE id = $1")
            .bind(citizen)
            .bind(b)
            .execute(&state.db)
            .await;
    }
    if let Some(b) = body.auto_federate_threshold {
        let _ = sqlx::query(r"UPDATE citizen SET auto_federate_threshold = $2 WHERE id = $1")
            .bind(citizen)
            .bind(b)
            .execute(&state.db)
            .await;
    }
    if let Some(ep) = body.email_prefs {
        // Shallow merge: only replaces the supplied keys.
        let _ = sqlx::query(
            r"UPDATE citizen
                 SET email_prefs = email_prefs || $2
               WHERE id = $1",
        )
        .bind(citizen)
        .bind(ep)
        .execute(&state.db)
        .await;
    }
    ok_json()
}

// ---------------------------------------------------------------------------
// Server rules
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, sqlx::FromRow)]
struct RuleDto {
    id: Uuid,
    ordinal: i32,
    text: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

async fn public_rules(State(state): State<AppState>) -> Response {
    let rows: Result<Vec<RuleDto>, _> = sqlx::query_as::<_, RuleDto>(
        r"SELECT id, ordinal, text, created_at, updated_at
            FROM server_rule
           ORDER BY ordinal, created_at",
    )
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(list) => (StatusCode::OK, Json(ApiResponse::ok(list))).into_response(),
        Err(err) => storage_err(err),
    }
}

async fn admin_list_rules(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    public_rules(State(state)).await
}

#[derive(Debug, Deserialize)]
struct RuleCreate {
    text: String,
    #[serde(default)]
    ordinal: i32,
}

async fn admin_create_rule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RuleCreate>,
) -> Response {
    let admin = match require_admin(&headers, &state.db).await {
        Ok(id) => id,
        Err(r) => return r,
    };
    let text = body.text.trim();
    if text.is_empty() || text.len() > 4000 {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::fail(
                "bad_request",
                "texto entre 1 e 4000 chars",
            )),
        )
            .into_response();
    }
    let res: Result<RuleDto, _> = sqlx::query_as::<_, RuleDto>(
        r"INSERT INTO server_rule (id, ordinal, text, created_by)
          VALUES ($1, $2, $3, $4)
          RETURNING id, ordinal, text, created_at, updated_at",
    )
    .bind(Uuid::now_v7())
    .bind(body.ordinal)
    .bind(text)
    .bind(admin)
    .fetch_one(&state.db)
    .await;
    match res {
        Ok(dto) => (StatusCode::OK, Json(ApiResponse::ok(dto))).into_response(),
        Err(err) => storage_err(err),
    }
}

#[derive(Debug, Deserialize)]
struct RuleUpdate {
    text: Option<String>,
    ordinal: Option<i32>,
}

async fn admin_update_rule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<RuleUpdate>,
) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    if let Some(t) = body.text.as_deref() {
        let _ = sqlx::query(r"UPDATE server_rule SET text = $2, updated_at = now() WHERE id = $1")
            .bind(id)
            .bind(t.trim())
            .execute(&state.db)
            .await;
    }
    if let Some(o) = body.ordinal {
        let _ =
            sqlx::query(r"UPDATE server_rule SET ordinal = $2, updated_at = now() WHERE id = $1")
                .bind(id)
                .bind(o)
                .execute(&state.db)
                .await;
    }
    ok_json()
}

async fn admin_delete_rule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    let _ = sqlx::query(r"DELETE FROM server_rule WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await;
    ok_json()
}

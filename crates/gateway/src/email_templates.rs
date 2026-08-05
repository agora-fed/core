//! Editable e-mail templates (0.25.0-fediverse).
//!
//! Every platform e-mail goes through here: the body is read from
//! `email_template` in the DB, `{{var}}` is substituted server-side with the context,
//! and the result goes to SMTP. The admin edits subject/body in the UI at
//! `/admin/email-templates`; reset to default via `PATCH /admin/email-templates/:key
//! {reset: true}` (subject/body ← default_subject/default_body).
//!
//! Template syntax: `{{var_name}}` only. No loops, no ifs, no HTML
//! escaping (every e-mail is text/plain). An unknown placeholder stays
//! literal as `{{foo}}` in the output — it signals to the admin that the variable is wrong.

use axum::extract::{Json, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch};
use axum::Router;
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Rendering — extracted to `dsoc_db::email_templates` (0.32.0) because the
// auth crate also renders (signup_verify/password_reset/mandate_invite)
// and cannot depend on the gateway. The re-export keeps callers here stable.
// ---------------------------------------------------------------------------

pub use dsoc_db::email_templates::{render, substitute};

// ---------------------------------------------------------------------------
// HTTP surface — admin CRUD
// ---------------------------------------------------------------------------

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/admin/email-templates", get(list))
        .route("/admin/email-templates/{key}", patch(update))
        .route(
            "/admin/email-templates/{key}/preview",
            axum::routing::post(preview),
        )
        // 0.32.1: sends the genuinely rendered template (multipart with the
        // brand's HTML wrapper) to a test mailbox — the admin validates the
        // real look without waiting for the event to happen.
        .route(
            "/admin/email-templates/{key}/send-test",
            axum::routing::post(send_test),
        )
        // GET /me/admin-status — used by the front end's AuthMenu to decide whether
        // to show the "Administration" link in the profile dropdown. Anonymous → 200
        // with `{is_admin: false}` (no signal leak). It is only here because the
        // path starts with /me instead of /admin — it matches require_admin below.
        .route("/me/admin-status", get(me_admin_status))
        .with_state(state)
}

/// `GET /me/admin-status` — lightweight; the AuthMenu calls it once at login and caches
/// it in localStorage's `dsoc_is_admin`. Returns `{is_admin: bool}`. Not a 401
/// for anonymous callers — it returns `false`, avoiding an error blip in the console
/// for non-admin users.
async fn me_admin_status(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(citizen_id): Option<Uuid> = headers
        .get("x-dsoc-citizen-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
    else {
        return (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "is_admin": false }))),
        )
            .into_response();
    };
    // Scoped to the caller's own org (issue #8): this probe drives whether the front
    // end renders admin controls, so an unscoped answer invites the user into screens
    // the API will refuse.
    let org_id = headers
        .get("x-dsoc-org-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<Uuid>().ok())
        .unwrap_or(crate::authz_ext::DEFAULT_ORG_UUID);
    let is_admin = sqlx::query_scalar::<_, bool>(
        r"SELECT EXISTS (
             SELECT 1 FROM admin_role_binding
              WHERE org_id = $1 AND citizen_id = $2 AND role IN ('owner','admin')
           )",
    )
    .bind(org_id)
    .bind(citizen_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(false);
    (
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({ "is_admin": is_admin }))),
    )
        .into_response()
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct TemplateRow {
    key: String,
    label: String,
    subject: String,
    body: String,
    default_subject: String,
    default_body: String,
    variables: Vec<String>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

/// Org-scoped admin gate — delegates to the single implementation in
/// [`crate::authz_ext::require_org_admin`] (issue #8). This module used to carry
/// its own copy that omitted `org_id`, so an owner of ANY org passed it.
async fn require_admin(headers: &HeaderMap, db: &PgPool) -> Result<Uuid, Response> {
    crate::authz_ext::require_org_admin(db, headers)
        .await
        .map(|a| a.citizen)
}

async fn list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_admin(&headers, &state.db).await {
        return resp;
    }
    match sqlx::query_as::<_, TemplateRow>(
        r"SELECT key, label, subject, body, default_subject, default_body,
                 variables, updated_at
            FROM email_template ORDER BY key",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => (StatusCode::OK, Json(ApiResponse::ok(rows))).into_response(),
        Err(err) => {
            tracing::error!(?err, "email_template list falhou");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::fail("storage_error", "Erro interno.")),
            )
                .into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct UpdateBody {
    /// `Some("")` clears it (uses the default). `None` leaves it as is.
    subject: Option<String>,
    body: Option<String>,
    /// `Some(true)` reseta subject+body pros defaults.
    #[serde(default)]
    reset: bool,
}

async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    Json(body): Json<UpdateBody>,
) -> Response {
    let admin_id = match require_admin(&headers, &state.db).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let res = if body.reset {
        sqlx::query(
            r"UPDATE email_template
                 SET subject = default_subject,
                     body    = default_body,
                     updated_at = now(),
                     updated_by = $2
               WHERE key = $1",
        )
        .bind(&key)
        .bind(admin_id)
        .execute(&state.db)
        .await
    } else {
        sqlx::query(
            r"UPDATE email_template
                 SET subject = COALESCE($2, subject),
                     body    = COALESCE($3, body),
                     updated_at = now(),
                     updated_by = $4
               WHERE key = $1",
        )
        .bind(&key)
        .bind(&body.subject)
        .bind(&body.body)
        .bind(admin_id)
        .execute(&state.db)
        .await
    };
    match res {
        Ok(r) if r.rows_affected() == 0 => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::fail("not_found", "Template não existe.")),
        )
            .into_response(),
        Ok(_) => (StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response(),
        Err(err) => {
            tracing::error!(?err, key, "email_template update falhou");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::fail("storage_error", "Erro interno.")),
            )
                .into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct SendTestBody {
    to: String,
    /// Sample values for the placeholders; when absent `{{var}}` stays literal.
    #[serde(default)]
    context: HashMap<String, String>,
}

/// `POST /admin/email-templates/{key}/send-test` — renders what is
/// SAVED and sends it to the given address through the real production path
/// (same SMTP, same HTML wrapper). The subject gains a `[TESTE]` prefix.
async fn send_test(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    Json(body): Json<SendTestBody>,
) -> Response {
    if let Err(resp) = require_admin(&headers, &state.db).await {
        return resp;
    }
    let to = body.to.trim();
    if to.len() < 5 || !to.contains('@') || to.contains(char::is_whitespace) {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::fail(
                "invalid_email",
                "Informe um e-mail de destino válido.",
            )),
        )
            .into_response();
    }
    let ctx: HashMap<&str, String> = body
        .context
        .iter()
        .map(|(k, v)| (k.as_str(), v.clone()))
        .collect();
    let Some((subject, rendered)) = render(&state.db, &key, &ctx).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::fail("not_found", "Template não existe.")),
        )
            .into_response();
    };
    let Some(cfg) = crate::proposal_delivery::smtp_from_env() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiResponse::<()>::fail(
                "smtp_unavailable",
                "SMTP não configurado neste ambiente.",
            )),
        )
            .into_response();
    };
    match crate::proposal_delivery::send_email(&cfg, to, &format!("[TESTE] {subject}"), &rendered)
        .await
    {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response(),
        Err(err) => {
            tracing::warn!(?err, key, "email_template send-test falhou");
            (
                StatusCode::BAD_GATEWAY,
                Json(ApiResponse::<()>::fail(
                    "smtp_error",
                    "O relay SMTP recusou o envio. Veja os logs.",
                )),
            )
                .into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct PreviewBody {
    /// Arbitrary context `{var_name: value}`. The UI passes the values the
    /// admin wants to test; an absent one stays literal as `{{var_name}}` in the output.
    #[serde(default)]
    context: HashMap<String, String>,
    /// When `Some(...)`, renders that draft subject/body (before saving).
    /// When `None`, renders what is saved today.
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    body: Option<String>,
}

#[derive(Debug, Serialize)]
struct PreviewResult {
    subject: String,
    body: String,
}

async fn preview(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    Json(body): Json<PreviewBody>,
) -> Response {
    if let Err(resp) = require_admin(&headers, &state.db).await {
        return resp;
    }
    // If the UI passed a draft subject/body, render that. Otherwise read from the DB.
    let (subject_tpl, body_tpl) =
        if body.subject.is_some() || body.body.is_some() {
            // We need the defaults to know what to render when a field is None.
            match sqlx::query_as::<_, TemplateRow>(
            "SELECT key, label, subject, body, default_subject, default_body, variables, updated_at
               FROM email_template WHERE key = $1",
        )
        .bind(&key)
        .fetch_optional(&state.db)
        .await
        {
            Ok(Some(row)) => (
                body.subject.unwrap_or(row.subject),
                body.body.unwrap_or(row.body),
            ),
            _ => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(ApiResponse::<()>::fail("not_found", "Template não existe.")),
                )
                    .into_response();
            }
        }
        } else {
            match sqlx::query_as::<_, TemplateRow>(
            "SELECT key, label, subject, body, default_subject, default_body, variables, updated_at
               FROM email_template WHERE key = $1",
        )
        .bind(&key)
        .fetch_optional(&state.db)
        .await
        {
            Ok(Some(row)) => (
                if row.subject.trim().is_empty() { row.default_subject } else { row.subject },
                if row.body.trim().is_empty() { row.default_body } else { row.body },
            ),
            _ => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(ApiResponse::<()>::fail("not_found", "Template não existe.")),
                )
                    .into_response();
            }
        }
        };
    let ctx: HashMap<&str, String> = body
        .context
        .iter()
        .map(|(k, v)| (k.as_str(), v.clone()))
        .collect();
    (
        StatusCode::OK,
        Json(ApiResponse::ok(PreviewResult {
            subject: substitute(&subject_tpl, &ctx),
            body: substitute(&body_tpl, &ctx),
        })),
    )
        .into_response()
}

// Tests of `substitute` moved along with the implementation to
// `dsoc_db::email_templates`.

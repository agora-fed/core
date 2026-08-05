//! Admin: the report queue (`note_report`).
//!
//! Closes the cycle opened by the "Report post" feature in the
//! three-dot menu: each report enters the table and here a moderator sees the queue,
//! can expand each one and mark it resolved with notes.
//!
//! - `GET  /admin/reports?status=pending|resolved&limit=&offset=` — lista
//!   with joins to hydrate the note's author and the reporter.
//! - `POST /admin/reports/{id}/resolve {notes?}` — marca resolved_at + notas.
//! - `POST /admin/reports/{id}/reopen` — undoes a resolution.
//!
//! Auth: `require_admin` (the same pattern as the other admin_* modules).

use axum::extract::{Json, Path, Query, State};
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

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/admin/reports", get(list_reports))
        .route("/admin/reports/{id}/resolve", post(resolve_report))
        .route("/admin/reports/{id}/reopen", post(reopen_report))
        // Moderation actions on accounts (slice 2).
        .route("/admin/accounts/{id}/suspend", post(suspend_account))
        .route("/admin/accounts/{id}/unsuspend", post(unsuspend_account))
        .route("/admin/accounts/{id}/silence", post(silence_account))
        .route("/admin/accounts/{id}/unsilence", post(unsilence_account))
        // Audit log.
        .route("/admin/audit", get(list_audit))
        // Server-wide federation.
        .route(
            "/admin/federation/domain_blocks",
            get(list_domain_blocks).post(add_domain_block_server),
        )
        .route(
            "/admin/federation/domain_blocks/{domain}",
            axum::routing::delete(remove_domain_block_server),
        )
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Guard (duplicated from the other admin_* modules: to avoid cross-coupling).
// ---------------------------------------------------------------------------

async fn require_admin(headers: &HeaderMap, db: &PgPool) -> Result<Uuid, Response> {
    let citizen_id: Uuid = headers
        .get("x-dsoc-citizen-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .ok_or_else(unauthorized_resp)?;
    let is_admin = sqlx::query_scalar::<_, bool>(
        r"SELECT EXISTS (
             SELECT 1 FROM admin_role_binding
              WHERE citizen_id = $1 AND role IN ('owner','admin')
           )",
    )
    .bind(citizen_id)
    .fetch_one(db)
    .await
    .unwrap_or(false);
    if !is_admin {
        return Err(forbidden_resp());
    }
    Ok(citizen_id)
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
fn storage_resp(err: impl std::fmt::Debug) -> Response {
    tracing::error!(?err, "admin_reports storage");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiResponse::<()>::fail("storage_error", "Erro interno.")),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// List
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ListQuery {
    /// pending (default) | resolved | all
    #[serde(default = "default_status")]
    status: String,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}
fn default_status() -> String {
    "pending".to_string()
}
fn default_limit() -> i64 {
    30
}

#[derive(Debug, Serialize)]
struct ReportDto {
    id: Uuid,
    object_uri: String,
    author_actor_url: String,
    category: String,
    reason: Option<String>,
    created_at: DateTime<Utc>,
    resolved_at: Option<DateTime<Utc>>,
    resolution_notes: Option<String>,
    reporter_handle: Option<String>,
    reporter_display_name: Option<String>,
    /// How many distinct reports that same note has already accumulated.
    total_for_note: i64,
}

async fn list_reports(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    let limit = q.limit.clamp(1, 200);
    let offset = q.offset.max(0);
    let where_clause = match q.status.as_str() {
        "resolved" => "WHERE nr.resolved_at IS NOT NULL",
        "all" => "",
        _ => "WHERE nr.resolved_at IS NULL",
    };
    let sql = format!(
        r"SELECT nr.id,
                 nr.object_uri,
                 nr.author_actor_url,
                 nr.category,
                 nr.reason,
                 nr.created_at,
                 nr.resolved_at,
                 nr.resolution_notes,
                 c.handle,
                 c.display_name,
                 (SELECT count(*) FROM note_report nr2
                   WHERE nr2.object_uri = nr.object_uri) AS total_for_note
            FROM note_report nr
            LEFT JOIN citizen c ON c.id = nr.reporter_id
            {where_clause}
           ORDER BY nr.created_at DESC
           LIMIT $1 OFFSET $2"
    );
    let rows = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            String,
            String,
            Option<String>,
            DateTime<Utc>,
            Option<DateTime<Utc>>,
            Option<String>,
            Option<String>,
            Option<String>,
            i64,
        ),
    >(&sql)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => {
            let list: Vec<ReportDto> = rows
                .into_iter()
                .map(
                    |(
                        id,
                        object_uri,
                        author_actor_url,
                        category,
                        reason,
                        created_at,
                        resolved_at,
                        resolution_notes,
                        handle,
                        display_name,
                        total_for_note,
                    )| ReportDto {
                        id,
                        object_uri,
                        author_actor_url,
                        category,
                        reason,
                        created_at,
                        resolved_at,
                        resolution_notes,
                        reporter_handle: handle,
                        reporter_display_name: display_name,
                        total_for_note,
                    },
                )
                .collect();
            (StatusCode::OK, Json(ApiResponse::ok(list))).into_response()
        }
        Err(err) => storage_resp(err),
    }
}

// ---------------------------------------------------------------------------
// Resolve
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
struct ResolveBody {
    #[serde(default)]
    notes: Option<String>,
}

async fn resolve_report(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    body: Option<Json<ResolveBody>>,
) -> Response {
    let admin_id = match require_admin(&headers, &state.db).await {
        Ok(id) => id,
        Err(r) => return r,
    };
    let notes = body
        .and_then(|Json(b)| b.notes)
        .map(|s| s.trim().to_owned());
    let notes = notes.filter(|s| !s.is_empty());
    let res = sqlx::query(
        r"UPDATE note_report
             SET resolved_at = now(),
                 resolved_by = $2,
                 resolution_notes = $3
           WHERE id = $1 AND resolved_at IS NULL",
    )
    .bind(id)
    .bind(admin_id)
    .bind(notes.as_deref())
    .execute(&state.db)
    .await;
    match res {
        Ok(r) if r.rows_affected() == 0 => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::fail(
                "not_found",
                "Denúncia não encontrada ou já resolvida.",
            )),
        )
            .into_response(),
        Ok(_) => {
            let _ = audit(
                &state.db,
                admin_id,
                "report_resolve",
                None,
                None,
                Some(id),
                notes.as_deref().map(|n| serde_json::json!({ "notes": n })),
            )
            .await;
            (
                StatusCode::OK,
                Json(ApiResponse::ok(serde_json::json!({ "ok": true }))),
            )
                .into_response()
        }
        Err(err) => storage_resp(err),
    }
}

async fn reopen_report(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let admin_id = match require_admin(&headers, &state.db).await {
        Ok(id) => id,
        Err(r) => return r,
    };
    let res = sqlx::query(
        r"UPDATE note_report
             SET resolved_at = NULL,
                 resolved_by = NULL,
                 resolution_notes = NULL
           WHERE id = $1 AND resolved_at IS NOT NULL",
    )
    .bind(id)
    .execute(&state.db)
    .await;
    match res {
        Ok(r) if r.rows_affected() > 0 => {
            let _ = audit(
                &state.db,
                admin_id,
                "report_reopen",
                None,
                None,
                Some(id),
                None,
            )
            .await;
            (
                StatusCode::OK,
                Json(ApiResponse::ok(serde_json::json!({ "ok": true }))),
            )
                .into_response()
        }
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "ok": true }))),
        )
            .into_response(),
        Err(err) => storage_resp(err),
    }
}

// ---------------------------------------------------------------------------
// Actions on accounts (slice 2)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
struct ModerateBody {
    #[serde(default)]
    reason: Option<String>,
}

async fn suspend_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    body: Option<Json<ModerateBody>>,
) -> Response {
    let admin_id = match require_admin(&headers, &state.db).await {
        Ok(id) => id,
        Err(r) => return r,
    };
    let reason = body.and_then(|Json(b)| b.reason).and_then(|s| {
        let t = s.trim().to_owned();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    });
    let res = sqlx::query(
        r"UPDATE citizen
             SET suspended_at = now(),
                 suspended_reason = $2
           WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(reason.as_deref())
    .execute(&state.db)
    .await;
    match res {
        Ok(r) if r.rows_affected() == 0 => not_found_json("Conta não encontrada."),
        Ok(_) => {
            // Active sessions: force logout (best-effort).
            let _ = sqlx::query(r"DELETE FROM auth_session WHERE citizen_id = $1")
                .bind(id)
                .execute(&state.db)
                .await;
            let _ = audit(
                &state.db,
                admin_id,
                "account_suspend",
                Some(id),
                None,
                None,
                reason
                    .as_deref()
                    .map(|s| serde_json::json!({ "reason": s })),
            )
            .await;
            crate::webhooks::dispatch_event(
                state.db.clone(),
                "account.suspended",
                serde_json::json!({ "citizen_id": id, "reason": reason }),
            );
            ok_json()
        }
        Err(err) => storage_resp(err),
    }
}

async fn unsuspend_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let admin_id = match require_admin(&headers, &state.db).await {
        Ok(id) => id,
        Err(r) => return r,
    };
    let res = sqlx::query(
        r"UPDATE citizen
             SET suspended_at = NULL,
                 suspended_reason = NULL
           WHERE id = $1",
    )
    .bind(id)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => {
            let _ = audit(
                &state.db,
                admin_id,
                "account_unsuspend",
                Some(id),
                None,
                None,
                None,
            )
            .await;
            ok_json()
        }
        Err(err) => storage_resp(err),
    }
}

async fn silence_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    body: Option<Json<ModerateBody>>,
) -> Response {
    let admin_id = match require_admin(&headers, &state.db).await {
        Ok(id) => id,
        Err(r) => return r,
    };
    let reason = body.and_then(|Json(b)| b.reason).and_then(|s| {
        let t = s.trim().to_owned();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    });
    let res = sqlx::query(
        r"UPDATE citizen
             SET silenced_at = now(),
                 silenced_reason = $2
           WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(reason.as_deref())
    .execute(&state.db)
    .await;
    match res {
        Ok(r) if r.rows_affected() == 0 => not_found_json("Conta não encontrada."),
        Ok(_) => {
            let _ = audit(
                &state.db,
                admin_id,
                "account_silence",
                Some(id),
                None,
                None,
                reason
                    .as_deref()
                    .map(|s| serde_json::json!({ "reason": s })),
            )
            .await;
            ok_json()
        }
        Err(err) => storage_resp(err),
    }
}

async fn unsilence_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let admin_id = match require_admin(&headers, &state.db).await {
        Ok(id) => id,
        Err(r) => return r,
    };
    let res = sqlx::query(
        r"UPDATE citizen
             SET silenced_at = NULL,
                 silenced_reason = NULL
           WHERE id = $1",
    )
    .bind(id)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => {
            let _ = audit(
                &state.db,
                admin_id,
                "account_unsilence",
                Some(id),
                None,
                None,
                None,
            )
            .await;
            ok_json()
        }
        Err(err) => storage_resp(err),
    }
}

// ---------------------------------------------------------------------------
// Audit log leitura
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct AuditRowDto {
    id: Uuid,
    admin_id: Uuid,
    admin_handle: Option<String>,
    action: String,
    target_citizen_id: Option<Uuid>,
    target_citizen_handle: Option<String>,
    target_domain: Option<String>,
    target_id: Option<Uuid>,
    detail: Option<serde_json::Value>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct AuditQuery {
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

async fn list_audit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<AuditQuery>,
) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    let limit = q.limit.clamp(1, 200);
    let offset = q.offset.max(0);
    let rows = sqlx::query_as::<
        _,
        (
            Uuid,
            Uuid,
            Option<String>,
            String,
            Option<Uuid>,
            Option<String>,
            Option<String>,
            Option<Uuid>,
            Option<serde_json::Value>,
            DateTime<Utc>,
        ),
    >(
        r"SELECT a.id,
                 a.admin_id,
                 (SELECT handle FROM citizen WHERE id = a.admin_id) AS admin_handle,
                 a.action,
                 a.target_citizen_id,
                 (SELECT handle FROM citizen WHERE id = a.target_citizen_id) AS target_citizen_handle,
                 a.target_domain,
                 a.target_id,
                 a.detail,
                 a.created_at
            FROM admin_audit a
           ORDER BY a.created_at DESC
           LIMIT $1 OFFSET $2",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => {
            let list: Vec<AuditRowDto> = rows
                .into_iter()
                .map(
                    |(
                        id,
                        admin_id,
                        admin_handle,
                        action,
                        target_citizen_id,
                        target_citizen_handle,
                        target_domain,
                        target_id,
                        detail,
                        created_at,
                    )| AuditRowDto {
                        id,
                        admin_id,
                        admin_handle,
                        action,
                        target_citizen_id,
                        target_citizen_handle,
                        target_domain,
                        target_id,
                        detail,
                        created_at,
                    },
                )
                .collect();
            (StatusCode::OK, Json(ApiResponse::ok(list))).into_response()
        }
        Err(err) => storage_resp(err),
    }
}

// ---------------------------------------------------------------------------
// Helpers compartilhados
// ---------------------------------------------------------------------------

fn ok_json() -> Response {
    (
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({ "ok": true }))),
    )
        .into_response()
}

fn not_found_json(msg: &'static str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(ApiResponse::<()>::fail("not_found", msg)),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Server-wide federation
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct DomainBlockDto {
    domain: String,
    severity: String,
    reason: Option<String>,
    created_at: DateTime<Utc>,
    created_by_handle: Option<String>,
}

async fn list_domain_blocks(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    let rows = sqlx::query_as::<
        _,
        (
            String,
            String,
            Option<String>,
            DateTime<Utc>,
            Option<String>,
        ),
    >(
        r"SELECT b.domain,
                 b.severity,
                 b.reason,
                 b.created_at,
                 (SELECT handle FROM citizen WHERE id = b.created_by) AS created_by_handle
            FROM server_domain_block b
           ORDER BY b.domain",
    )
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => {
            let list: Vec<DomainBlockDto> = rows
                .into_iter()
                .map(
                    |(domain, severity, reason, created_at, created_by_handle)| DomainBlockDto {
                        domain,
                        severity,
                        reason,
                        created_at,
                        created_by_handle,
                    },
                )
                .collect();
            (StatusCode::OK, Json(ApiResponse::ok(list))).into_response()
        }
        Err(err) => storage_resp(err),
    }
}

#[derive(Debug, Deserialize)]
struct AddDomainBlockBody {
    domain: String,
    severity: String,
    #[serde(default)]
    reason: Option<String>,
}

fn normalize_domain(raw: &str) -> Option<String> {
    let d = raw.trim().to_ascii_lowercase();
    let host = if let Some(rest) = d
        .strip_prefix("https://")
        .or_else(|| d.strip_prefix("http://"))
    {
        rest.split('/').next().unwrap_or("")
    } else {
        d.as_str()
    };
    let host = host.split(':').next().unwrap_or("");
    if host.is_empty() || !host.contains('.') || host.len() > 253 {
        None
    } else {
        Some(host.to_owned())
    }
}

async fn add_domain_block_server(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AddDomainBlockBody>,
) -> Response {
    let admin_id = match require_admin(&headers, &state.db).await {
        Ok(id) => id,
        Err(r) => return r,
    };
    let Some(domain) = normalize_domain(&body.domain) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::fail("bad_request", "domínio inválido")),
        )
            .into_response();
    };
    if !matches!(body.severity.as_str(), "silence" | "suspend") {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::fail(
                "bad_request",
                "severity deve ser silence ou suspend",
            )),
        )
            .into_response();
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
        r"INSERT INTO server_domain_block (id, domain, severity, reason, created_by)
          VALUES ($1, $2, $3, $4, $5)
          ON CONFLICT (domain)
          DO UPDATE SET severity = EXCLUDED.severity,
                        reason   = EXCLUDED.reason,
                        created_by = EXCLUDED.created_by",
    )
    .bind(Uuid::now_v7())
    .bind(&domain)
    .bind(&body.severity)
    .bind(reason.as_deref())
    .bind(admin_id)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => {
            let _ = audit(
                &state.db,
                admin_id,
                "server_domain_block",
                None,
                Some(&domain),
                None,
                Some(serde_json::json!({ "severity": body.severity, "reason": reason })),
            )
            .await;
            ok_json()
        }
        Err(err) => storage_resp(err),
    }
}

async fn remove_domain_block_server(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(domain): Path<String>,
) -> Response {
    let admin_id = match require_admin(&headers, &state.db).await {
        Ok(id) => id,
        Err(r) => return r,
    };
    let Some(domain) = normalize_domain(&domain) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::fail("bad_request", "domínio inválido")),
        )
            .into_response();
    };
    let _ = sqlx::query(r"DELETE FROM server_domain_block WHERE domain = $1")
        .bind(&domain)
        .execute(&state.db)
        .await;
    let _ = audit(
        &state.db,
        admin_id,
        "server_domain_unblock",
        None,
        Some(&domain),
        None,
        None,
    )
    .await;
    ok_json()
}

async fn audit(
    db: &PgPool,
    admin_id: Uuid,
    action: &str,
    target_citizen_id: Option<Uuid>,
    target_domain: Option<&str>,
    target_id: Option<Uuid>,
    detail: Option<serde_json::Value>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r"INSERT INTO admin_audit
            (id, admin_id, action, target_citizen_id, target_domain, target_id, detail)
          VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(Uuid::now_v7())
    .bind(admin_id)
    .bind(action)
    .bind(target_citizen_id)
    .bind(target_domain)
    .bind(target_id)
    .bind(detail)
    .execute(db)
    .await
    .map(|_| ())
}

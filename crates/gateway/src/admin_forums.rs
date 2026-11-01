//! # Forum configuration (F3) — admin panel.
//!
//! Where the platform admin curates each forum's **institutional e-mail**, its
//! dispatch **thresholds** and its **moderators**. Runtime queries (the
//! politico_contacts pattern). The postman that consumes pending `forum_dispatch` rows lives
//! em [`crate::forum_mailer`].
//!
//! - `GET    /admin/forums?q=&limit=&offset=`      — a list with e-mail + pending items.
//! - `PATCH  /admin/forums/{id}`                   — contact_email / thresholds.
//! - `GET    /admin/forums/{id}/moderators`        — list moderators (handle).
//! - `POST   /admin/forums/{id}/moderators`        — add by handle.
//! - `DELETE /admin/forums/{id}/moderators/{cid}`  — remove.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use dsoc_api_contract::ApiResponse;
use dsoc_app::{AppState, CallerId};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/admin/forums", get(list).post(create))
        .route("/admin/forums/{id}", patch(update).delete(archive))
        .route(
            "/admin/forums/{id}/members",
            get(members_list).post(members_add),
        )
        .route("/admin/forums/{id}/members/{cid}", delete(members_remove))
        .route(
            "/admin/forums/{id}/moderators",
            get(mods_list).post(mods_add),
        )
        .route("/admin/forums/{id}/moderators/{cid}", delete(mods_remove))
        // Content moderation (R3.1 #27): a global admin (content.moderate) OR
        // a forum moderator removes a topic/argument. Soft-delete + audit.
        .route("/f/topics/{id}/remove", post(topic_remove))
        .route("/f/comments/{id}/remove", post(comment_remove))
        .with_state(state)
}

fn fail(status: StatusCode, code: &str, msg: &str) -> Response {
    (status, Json(ApiResponse::<()>::fail(code, msg))).into_response()
}
fn storage_error() -> Response {
    fail(
        StatusCode::INTERNAL_SERVER_ERROR,
        "storage_error",
        "Erro interno.",
    )
}
/// Org-scoped admin gate — delegates to the single implementation in
/// [`crate::authz_ext::require_org_admin`] (issue #8). This module used to carry
/// its own copy that omitted `org_id`, so an owner of ANY org passed it.
async fn require_admin(db: &PgPool, headers: &HeaderMap) -> Result<(), Response> {
    crate::authz_ext::require_org_admin(db, headers)
        .await
        .map(|_| ())
}

#[derive(Debug, Deserialize)]
struct ListParams {
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct AdminForumRow {
    id: Uuid,
    full_path: String,
    name: String,
    kind: String,
    esfera: Option<String>,
    contact_email: Option<String>,
    avatar_url: Option<String>,
    banner_url: Option<String>,
    thresholds: Vec<i32>,
    visibility: String,
    federated: bool,
    member_count: i64,
    moderator_count: i64,
    pending_dispatches: i64,
    topic_count: i64,
}

async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(p): Query<ListParams>,
) -> Response {
    if let Err(r) = require_admin(&state.db, &headers).await {
        return r;
    }
    let q = p.q.unwrap_or_default();
    let like = format!("%{}%", q.trim().to_lowercase());
    let rows: Result<Vec<AdminForumRow>, _> = sqlx::query_as(
        r"SELECT f.id, f.full_path, f.name, f.kind, f.esfera, f.contact_email,
                 f.avatar_url, f.banner_url, f.thresholds, f.visibility, f.federated,
                 (SELECT count(*) FROM forum_member m WHERE m.forum_id = f.id) AS member_count,
                 (SELECT count(*) FROM forum_moderator m WHERE m.forum_id = f.id) AS moderator_count,
                 (SELECT count(*) FROM forum_dispatch d JOIN forum_topic t ON t.id = d.topic_id
                   WHERE t.forum_id = f.id AND d.sent_at IS NULL) AS pending_dispatches,
                 (SELECT count(*) FROM forum_topic t WHERE t.forum_id = f.id) AS topic_count
            FROM forum f
           WHERE f.hidden_at IS NULL
             AND ($1 = '%%' OR lower(f.full_path) LIKE $1 OR lower(f.name) LIKE $1)
           ORDER BY f.full_path
           LIMIT $2 OFFSET $3",
    )
    .bind(&like)
    .bind(p.limit.unwrap_or(50).clamp(1, 200))
    .bind(p.offset.unwrap_or(0).max(0))
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => (StatusCode::OK, Json(ApiResponse::ok(rows))).into_response(),
        Err(err) => {
            tracing::warn!(?err, "admin_forums: list falhou");
            storage_error()
        }
    }
}

#[derive(Debug, Deserialize)]
struct UpdateForumRequest {
    /// `Some("a@b")` sets it; `Some("")` clears it; `None` keeps it.
    contact_email: Option<String>,
    /// New thresholds (ascending, positive, max 10). `None` keeps them.
    thresholds: Option<Vec<i32>>,
    /// Logo — `Some("")` clears it; `None` keeps it.
    avatar_url: Option<String>,
    /// Banner — `Some("")` clears it; `None` keeps it.
    banner_url: Option<String>,
    /// `public` | `members` | `list`. Restricting a forum also stops its federation.
    visibility: Option<String>,
    /// Only a public forum may federate.
    federated: Option<bool>,
}

fn valid_visibility(value: &str) -> bool {
    matches!(value, "public" | "members" | "list")
}

/// The slug becomes both a URL and a fediverse handle. `topico` is a route under `/f/`, and
/// two-letter slugs are reserved for states in the territorial tree.
fn valid_slug(slug: &str) -> bool {
    (3..=79).contains(&slug.len())
        && slug != "topico"
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !slug.starts_with('-')
}

#[derive(Debug, Deserialize)]
struct CreateForumRequest {
    slug: String,
    name: String,
    #[serde(default)]
    description: String,
    visibility: Option<String>,
    #[serde(default)]
    federated: bool,
}

/// `POST /admin/forums` — a new community forum at the root of the tree.
async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateForumRequest>,
) -> Response {
    let admin = match crate::authz_ext::require_org_admin(&state.db, &headers).await {
        Ok(admin) => admin,
        Err(denied) => return denied,
    };
    let slug = req.slug.trim().to_lowercase();
    let name = req.name.trim();
    let visibility = req.visibility.as_deref().unwrap_or("public");
    if !valid_slug(&slug) || name.is_empty() || name.chars().count() > 120 {
        return fail(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_input",
            "Use um endereço com letras minúsculas, números e hífens (mínimo 3) e um nome de até 120 caracteres.",
        );
    }
    if !valid_visibility(visibility) || (req.federated && visibility != "public") {
        return fail(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_input",
            "Só um fórum público pode ser federado.",
        );
    }
    let id = Uuid::now_v7();
    let res = sqlx::query(
        r"INSERT INTO forum (id, org_id, slug, full_path, name, description, kind, federated,
                             visibility, created_by)
          VALUES ($1, $2, $3, $3, $4, $5, 'comunitario', $6, $7, $8)
          ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(admin.org)
    .bind(&slug)
    .bind(name)
    .bind(req.description.trim())
    .bind(req.federated)
    .bind(visibility)
    .bind(admin.citizen)
    .execute(&state.db)
    .await;
    match res {
        Ok(r) if r.rows_affected() == 1 => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(
                serde_json::json!({"id": id, "full_path": slug}),
            )),
        )
            .into_response(),
        Ok(_) => fail(
            StatusCode::CONFLICT,
            "conflict",
            "Já existe um fórum com esse endereço.",
        ),
        Err(err) => {
            tracing::warn!(?err, "admin_forums: create falhou");
            storage_error()
        }
    }
}

/// `DELETE /admin/forums/{id}` — archive: leaves every listing and its topics stop being
/// readable. Rows are kept for the record.
async fn archive(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let admin = match crate::authz_ext::require_org_admin(&state.db, &headers).await {
        Ok(admin) => admin,
        Err(denied) => return denied,
    };
    let res = sqlx::query(
        "UPDATE forum SET hidden_at = now() WHERE id = $1 AND org_id = $2 AND hidden_at IS NULL",
    )
    .bind(id)
    .bind(admin.org)
    .execute(&state.db)
    .await;
    match res {
        Ok(r) if r.rows_affected() == 1 => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({"ok": true}))),
        )
            .into_response(),
        Ok(_) => fail(StatusCode::NOT_FOUND, "not_found", "Fórum não encontrado."),
        Err(err) => {
            tracing::warn!(?err, "admin_forums: archive falhou");
            storage_error()
        }
    }
}

async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateForumRequest>,
) -> Response {
    if let Err(r) = require_admin(&state.db, &headers).await {
        return r;
    }
    if req
        .visibility
        .as_deref()
        .is_some_and(|v| !valid_visibility(v))
    {
        return fail(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_input",
            "Nível de acesso inválido.",
        );
    }
    if let Some(ts) = &req.thresholds {
        let ok = !ts.is_empty()
            && ts.len() <= 10
            && ts.iter().all(|t| *t > 0)
            && ts.windows(2).all(|w| w[0] < w[1]);
        if !ok {
            return fail(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_input",
                "Patamares devem ser positivos, crescentes e no máximo 10.",
            );
        }
    }
    if let Some(email) = &req.contact_email {
        let e = email.trim();
        if !e.is_empty() && (!e.contains('@') || e.contains(' ')) {
            return fail(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_input",
                "E-mail institucional inválido.",
            );
        }
    }
    let restricted_federation = req.federated == Some(true);
    let email_update = req.contact_email.is_some();
    let email_value: Option<String> = req.contact_email.and_then(|e| {
        let e = e.trim().to_owned();
        if e.is_empty() {
            None
        } else {
            Some(e)
        }
    });
    let norm = |v: Option<String>| -> (bool, Option<String>) {
        match v {
            None => (false, None),
            Some(s) => {
                let s = s.trim().to_owned();
                (true, if s.is_empty() { None } else { Some(s) })
            }
        }
    };
    let (av_up, av_val) = norm(req.avatar_url);
    let (bn_up, bn_val) = norm(req.banner_url);
    let res = sqlx::query(
        r"UPDATE forum SET
             contact_email = CASE WHEN $2 THEN $3 ELSE contact_email END,
             thresholds    = COALESCE($4, thresholds),
             avatar_url    = CASE WHEN $5 THEN $6 ELSE avatar_url END,
             banner_url    = CASE WHEN $7 THEN $8 ELSE banner_url END,
             visibility    = COALESCE($9, visibility),
             federated     = CASE WHEN COALESCE($9, visibility) = 'public'
                                  THEN COALESCE($10, federated) ELSE false END
           WHERE id = $1
             AND NOT ($10 IS TRUE AND COALESCE($9, visibility) <> 'public')",
    )
    .bind(id)
    .bind(email_update)
    .bind(email_value)
    .bind(req.thresholds)
    .bind(av_up)
    .bind(av_val)
    .bind(bn_up)
    .bind(bn_val)
    .bind(req.visibility)
    .bind(req.federated)
    .execute(&state.db)
    .await;
    match res {
        Ok(r) if r.rows_affected() == 1 => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({"ok": true}))),
        )
            .into_response(),
        Ok(_) if restricted_federation => fail(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_input",
            "Só um fórum público pode ser federado.",
        ),
        Ok(_) => fail(StatusCode::NOT_FOUND, "not_found", "Fórum não encontrado."),
        Err(err) => {
            tracing::warn!(?err, "admin_forums: update falhou");
            storage_error()
        }
    }
}

/// The forum, when it belongs to the admin's org.
async fn own_forum(db: &PgPool, org: Uuid, id: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM forum WHERE id = $1 AND org_id = $2)")
        .bind(id)
        .bind(org)
        .fetch_one(db)
        .await
}

/// `GET /admin/forums/{id}/members`.
async fn members_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let admin = match crate::authz_ext::require_org_admin(&state.db, &headers).await {
        Ok(admin) => admin,
        Err(denied) => return denied,
    };
    let rows: Result<Vec<ModeratorRow>, _> = sqlx::query_as(
        r"SELECT m.citizen_id, c.handle, c.display_name
            FROM forum_member m
            JOIN citizen c ON c.id = m.citizen_id
            JOIN forum f ON f.id = m.forum_id
           WHERE m.forum_id = $1 AND f.org_id = $2
           ORDER BY c.display_name NULLS LAST, c.handle",
    )
    .bind(id)
    .bind(admin.org)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => (StatusCode::OK, Json(ApiResponse::ok(rows))).into_response(),
        Err(err) => {
            tracing::warn!(?err, "admin_forums: members_list falhou");
            storage_error()
        }
    }
}

#[derive(Debug, Deserialize)]
struct AddMemberRequest {
    citizen_id: Uuid,
}

/// `POST /admin/forums/{id}/members` — the id comes from the person search on the admin page.
async fn members_add(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(req): Json<AddMemberRequest>,
) -> Response {
    let admin = match crate::authz_ext::require_org_admin(&state.db, &headers).await {
        Ok(admin) => admin,
        Err(denied) => return denied,
    };
    if !matches!(own_forum(&state.db, admin.org, id).await, Ok(true)) {
        return fail(StatusCode::NOT_FOUND, "not_found", "Fórum não encontrado.");
    }
    let res = sqlx::query(
        r"INSERT INTO forum_member (forum_id, citizen_id, added_by)
          SELECT $1, c.id, $3 FROM citizen c WHERE c.id = $2 AND c.org_id = $4
          ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(req.citizen_id)
    .bind(admin.citizen)
    .bind(admin.org)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(serde_json::json!({"ok": true}))),
        )
            .into_response(),
        Err(err) => {
            tracing::warn!(?err, "admin_forums: members_add falhou");
            storage_error()
        }
    }
}

/// `DELETE /admin/forums/{id}/members/{cid}`.
async fn members_remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, citizen_id)): Path<(Uuid, Uuid)>,
) -> Response {
    let admin = match crate::authz_ext::require_org_admin(&state.db, &headers).await {
        Ok(admin) => admin,
        Err(denied) => return denied,
    };
    if !matches!(own_forum(&state.db, admin.org, id).await, Ok(true)) {
        return fail(StatusCode::NOT_FOUND, "not_found", "Fórum não encontrado.");
    }
    let res = sqlx::query("DELETE FROM forum_member WHERE forum_id = $1 AND citizen_id = $2")
        .bind(id)
        .bind(citizen_id)
        .execute(&state.db)
        .await;
    match res {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({"ok": true}))),
        )
            .into_response(),
        Err(err) => {
            tracing::warn!(?err, "admin_forums: members_remove falhou");
            storage_error()
        }
    }
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct ModeratorRow {
    citizen_id: Uuid,
    handle: Option<String>,
    display_name: Option<String>,
}

async fn mods_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    if let Err(r) = require_admin(&state.db, &headers).await {
        return r;
    }
    let rows: Result<Vec<ModeratorRow>, _> = sqlx::query_as(
        r"SELECT m.citizen_id, c.handle, c.display_name
            FROM forum_moderator m JOIN citizen c ON c.id = m.citizen_id
           WHERE m.forum_id = $1 ORDER BY m.created_at",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => (StatusCode::OK, Json(ApiResponse::ok(rows))).into_response(),
        Err(err) => {
            tracing::warn!(?err, "admin_forums: mods_list falhou");
            storage_error()
        }
    }
}

#[derive(Debug, Deserialize)]
struct AddModeratorRequest {
    handle: String,
}

async fn mods_add(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(req): Json<AddModeratorRequest>,
) -> Response {
    if let Err(r) = require_admin(&state.db, &headers).await {
        return r;
    }
    let handle = req.handle.trim().trim_start_matches('@').to_lowercase();
    let citizen: Option<Uuid> = sqlx::query_scalar("SELECT id FROM citizen WHERE handle = $1")
        .bind(&handle)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);
    let Some(citizen) = citizen else {
        return fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "Nenhum cidadão com esse @handle.",
        );
    };
    let res = sqlx::query(
        "INSERT INTO forum_moderator (forum_id, citizen_id) VALUES ($1, $2)
         ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(citizen)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(serde_json::json!({"citizen_id": citizen}))),
        )
            .into_response(),
        Err(err) => {
            tracing::warn!(?err, "admin_forums: mods_add falhou");
            storage_error()
        }
    }
}

async fn mods_remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, citizen_id)): Path<(Uuid, Uuid)>,
) -> Response {
    if let Err(r) = require_admin(&state.db, &headers).await {
        return r;
    }
    let res = sqlx::query("DELETE FROM forum_moderator WHERE forum_id = $1 AND citizen_id = $2")
        .bind(id)
        .bind(citizen_id)
        .execute(&state.db)
        .await;
    match res {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({"ok": true}))),
        )
            .into_response(),
        Err(err) => {
            tracing::warn!(?err, "admin_forums: mods_remove falhou");
            storage_error()
        }
    }
}

// ---------------------------------------------------------------------------
// Content moderation (R3.1 #27)
// ---------------------------------------------------------------------------

/// Optional body carrying the removal reason (it goes to the audit and to deletion_reason).
#[derive(Debug, Default, Deserialize)]
struct RemoveBody {
    #[serde(default)]
    reason: Option<String>,
}

/// May they moderate this forum? The global `content.moderate`/`forums.moderate` permission
/// (via `require_permission`), OR a designated moderator of this forum (0541). It is the point
/// where the configurable Moderator role and the per-forum moderator converge.
async fn can_moderate_forum(state: &AppState, caller: CallerId, forum_id: Uuid) -> bool {
    let svc = dsoc_admin::AdminService::from_state(state);
    if let Ok(perms) = svc.permissions_for(caller.org, caller.citizen).await {
        if perms.can("content.moderate") || perms.can("forums.moderate") {
            return true;
        }
    }
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM forum_moderator WHERE forum_id = $1 AND citizen_id = $2)",
    )
    .bind(forum_id)
    .bind(caller.citizen.as_uuid())
    .fetch_one(&state.db)
    .await
    .unwrap_or(false)
}

/// Someone who can't read a restricted forum must not learn that the content exists.
async fn cannot_moderate(db: &PgPool, caller: CallerId, forum_id: Uuid) -> Response {
    let readable: bool = sqlx::query_scalar("SELECT forum_readable($1, $2)")
        .bind(forum_id)
        .bind(caller.citizen.as_uuid())
        .fetch_one(db)
        .await
        .unwrap_or(false);
    if readable {
        fail(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Você não pode moderar este fórum.",
        )
    } else {
        fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "Conteúdo não encontrado.",
        )
    }
}

fn trim_reason(reason: Option<String>) -> Option<String> {
    reason
        .map(|s| s.trim().chars().take(2000).collect::<String>())
        .filter(|s| !s.is_empty())
}

/// Tell a forum's followers about a removal. Never fails the moderation that already happened.
async fn announce_removal(db: &PgPool, kind: &str, id: Uuid, reason: Option<&str>) {
    let Some(host) = std::env::var("PUBLIC_ORIGIN")
        .ok()
        .and_then(|origin| crate::forum_inbound::https_host(&origin))
    else {
        return;
    };
    if let Err(err) = crate::forum_outbound::enqueue_removal(db, &host, kind, id, reason).await {
        tracing::warn!(?err, "forum removal: queueing the announcement failed");
    }
}

async fn topic_remove(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<Uuid>,
    body: Option<Json<RemoveBody>>,
) -> Response {
    let forum_id: Option<Uuid> =
        sqlx::query_scalar("SELECT forum_id FROM forum_topic WHERE id = $1 AND hidden_at IS NULL")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    let Some(forum_id) = forum_id else {
        return fail(StatusCode::NOT_FOUND, "not_found", "Tópico não encontrado.");
    };
    // If the org switched the forums module off, the route "does not exist" (R0.5).
    if let Err(r) = crate::module_gate::require_module(&state, caller.org.as_uuid(), "forums").await
    {
        return r;
    }
    if !can_moderate_forum(&state, caller, forum_id).await {
        return cannot_moderate(&state.db, caller, forum_id).await;
    }
    let reason = trim_reason(body.map(|b| b.0).unwrap_or_default().reason);
    let res = sqlx::query(
        "UPDATE forum_topic SET hidden_at = now(), deleted_by = $2, deletion_reason = $3 \
         WHERE id = $1",
    )
    .bind(id)
    .bind(caller.citizen.as_uuid())
    .bind(reason.as_deref())
    .execute(&state.db)
    .await;
    if let Err(err) = res {
        tracing::warn!(?err, "topic_remove falhou");
        return storage_error();
    }
    let _ = audit_moderation(
        &state.db,
        caller,
        "forum.topic.remove",
        id,
        reason.as_deref(),
    )
    .await;
    announce_removal(&state.db, "topic", id, reason.as_deref()).await;
    (
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({"removed": true}))),
    )
        .into_response()
}

async fn comment_remove(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<Uuid>,
    body: Option<Json<RemoveBody>>,
) -> Response {
    // Find the forum through the argument's topic.
    let forum_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT t.forum_id FROM forum_topic_comment c \
           JOIN forum_topic t ON t.id = c.topic_id \
          WHERE c.id = $1 AND c.hidden_at IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();
    let Some(forum_id) = forum_id else {
        return fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "Argumento não encontrado.",
        );
    };
    if let Err(r) = crate::module_gate::require_module(&state, caller.org.as_uuid(), "forums").await
    {
        return r;
    }
    if !can_moderate_forum(&state, caller, forum_id).await {
        return cannot_moderate(&state.db, caller, forum_id).await;
    }
    let reason = trim_reason(body.map(|b| b.0).unwrap_or_default().reason);
    let res = sqlx::query(
        "UPDATE forum_topic_comment SET hidden_at = now(), deleted_by = $2, deletion_reason = $3 \
         WHERE id = $1",
    )
    .bind(id)
    .bind(caller.citizen.as_uuid())
    .bind(reason.as_deref())
    .execute(&state.db)
    .await;
    if let Err(err) = res {
        tracing::warn!(?err, "comment_remove falhou");
        return storage_error();
    }
    let _ = audit_moderation(
        &state.db,
        caller,
        "forum.comment.remove",
        id,
        reason.as_deref(),
    )
    .await;
    announce_removal(&state.db, "comment", id, reason.as_deref()).await;
    (
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({"removed": true}))),
    )
        .into_response()
}

/// Record the moderation action in `admin_audit` (the same table as the admin_* ones).
async fn audit_moderation(
    db: &PgPool,
    caller: CallerId,
    action: &str,
    target_id: Uuid,
    reason: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r"INSERT INTO admin_audit
            (id, admin_id, action, target_citizen_id, target_domain, target_id, detail)
          VALUES ($1, $2, $3, NULL, 'forums', $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(caller.citizen.as_uuid())
    .bind(action)
    .bind(target_id)
    .bind(reason.map(|r| serde_json::json!({ "reason": r })))
    .execute(db)
    .await
    .map(|_| ())
}

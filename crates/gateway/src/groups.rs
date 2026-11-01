//! # Groups: working groups of a community, an optional module that is off by default.
//!
//! Who belongs to a group can be sensitive, so everything requires a session, and only
//! active members and org admins see the member list. A group's forum uses the `list` access
//! level, with `forum_member` kept in step with the group's active members.

use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use dsoc_api_contract::ApiResponse;
use dsoc_app::{AppState, CallerId};
use dsoc_core::VerificationLevel;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::admin_branding::DEFAULT_ORG_UUID;

pub(crate) fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/groups", get(list).post(create))
        .route("/groups/by-slug/{slug}", get(detail))
        .route("/groups/{id}", patch(update).delete(archive))
        .route("/groups/{id}/join", post(join).delete(leave))
        .route("/groups/{id}/members", get(members).post(member_add))
        .route(
            "/groups/{id}/members/{citizen}",
            patch(member_update).delete(member_remove),
        )
        .route("/groups/{id}/people", get(people))
        .route("/groups/{id}/forum", post(enable_forum))
        // A middleware, so a switched-off module answers 404 before asking anyone to sign in.
        .route_layer(middleware::from_fn_with_state(state.clone(), module_gate))
        .with_state(state)
}

async fn module_gate(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let org = req
        .headers()
        .get("x-dsoc-org-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_ORG_UUID);
    match crate::module_gate::require_module(&state, org, "groups").await {
        Ok(()) => next.run(req).await,
        Err(off) => off,
    }
}

fn fail(status: StatusCode, code: &str, msg: &str) -> Response {
    (status, Json(ApiResponse::<()>::fail(code, msg))).into_response()
}
fn not_found() -> Response {
    fail(StatusCode::NOT_FOUND, "not_found", "Grupo não encontrado.")
}
fn forbidden() -> Response {
    fail(
        StatusCode::FORBIDDEN,
        "forbidden",
        "Só a coordenação do grupo pode fazer isso.",
    )
}
fn invalid(msg: &str) -> Response {
    fail(StatusCode::UNPROCESSABLE_ENTITY, "invalid_input", msg)
}
fn storage(err: &sqlx::Error) -> Response {
    tracing::warn!(?err, "groups: storage error");
    fail(
        StatusCode::INTERNAL_SERVER_ERROR,
        "storage_error",
        "Erro interno.",
    )
}
fn ok_flag() -> Response {
    (
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({"ok": true}))),
    )
        .into_response()
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct GroupRow {
    id: Uuid,
    slug: String,
    name: String,
    description: String,
    entry: String,
    forum_path: Option<String>,
    member_count: i64,
    my_status: Option<String>,
    my_role: Option<String>,
}

const GROUP_SELECT: &str = r"
    SELECT g.id, g.slug, g.name, g.description, g.entry,
           (SELECT f.full_path FROM forum f WHERE f.id = g.forum_id AND f.hidden_at IS NULL) AS forum_path,
           (SELECT count(*) FROM community_group_member m
             WHERE m.group_id = g.id AND m.status = 'active') AS member_count,
           me.status AS my_status, me.role AS my_role
      FROM community_group g
      LEFT JOIN community_group_member me ON me.group_id = g.id AND me.citizen_id = $2
     WHERE g.org_id = $1 AND g.archived_at IS NULL";

async fn is_org_admin(db: &PgPool, caller: CallerId) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM admin_role_binding
          WHERE org_id = $1 AND citizen_id = $2 AND role IN ('owner', 'admin'))",
    )
    .bind(caller.org.as_uuid())
    .bind(caller.citizen.as_uuid())
    .fetch_one(db)
    .await
}

/// The caller's standing in a live group of their org; `None` when there is no such group.
struct Standing {
    entry: String,
    forum_id: Option<Uuid>,
    status: Option<String>,
    role: Option<String>,
    admin: bool,
}

impl Standing {
    fn active_member(&self) -> bool {
        self.status.as_deref() == Some("active")
    }
    fn manager(&self) -> bool {
        self.admin || (self.active_member() && self.role.as_deref() == Some("coordinator"))
    }
}

async fn standing(
    db: &PgPool,
    caller: CallerId,
    id: Uuid,
) -> Result<Option<Standing>, sqlx::Error> {
    let row: Option<(String, Option<Uuid>, Option<String>, Option<String>)> = sqlx::query_as(
        r"SELECT g.entry, g.forum_id, me.status, me.role
            FROM community_group g
            LEFT JOIN community_group_member me ON me.group_id = g.id AND me.citizen_id = $3
           WHERE g.id = $1 AND g.org_id = $2 AND g.archived_at IS NULL",
    )
    .bind(id)
    .bind(caller.org.as_uuid())
    .bind(caller.citizen.as_uuid())
    .fetch_optional(db)
    .await?;
    let Some((entry, forum_id, status, role)) = row else {
        return Ok(None);
    };
    Ok(Some(Standing {
        entry,
        forum_id,
        status,
        role,
        admin: is_org_admin(db, caller).await?,
    }))
}

/// Load the group or answer for the caller: 404 when absent, 403 when not a manager.
async fn managed(state: &AppState, caller: CallerId, id: Uuid) -> Result<Standing, Response> {
    match standing(&state.db, caller, id).await {
        Ok(Some(s)) if s.manager() => Ok(s),
        Ok(Some(_)) => Err(forbidden()),
        Ok(None) => Err(not_found()),
        Err(err) => Err(storage(&err)),
    }
}

/// Keep the private forum's list equal to the group's active members.
async fn sync_forum(
    tx: &mut Transaction<'_, Postgres>,
    forum: Option<Uuid>,
    citizen: Uuid,
    active: bool,
) -> Result<(), sqlx::Error> {
    let Some(forum) = forum else {
        return Ok(());
    };
    let sql = if active {
        "INSERT INTO forum_member (forum_id, citizen_id) VALUES ($1, $2) ON CONFLICT DO NOTHING"
    } else {
        "DELETE FROM forum_member WHERE forum_id = $1 AND citizen_id = $2"
    };
    sqlx::query(sql)
        .bind(forum)
        .bind(citizen)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Serialize changes to a group's coordinators, so two removals can't both pass the check
/// and leave it with none.
async fn lock_group(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT id FROM community_group WHERE id = $1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?;
    Ok(())
}

async fn is_last_coordinator(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    citizen: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        r"SELECT EXISTS (SELECT 1 FROM community_group_member
            WHERE group_id = $1 AND citizen_id = $2 AND role = 'coordinator' AND status = 'active')
          AND NOT EXISTS (SELECT 1 FROM community_group_member
            WHERE group_id = $1 AND citizen_id <> $2 AND role = 'coordinator' AND status = 'active')",
    )
    .bind(id)
    .bind(citizen)
    .fetch_one(&mut **tx)
    .await
}

fn last_coordinator() -> Response {
    fail(
        StatusCode::CONFLICT,
        "last_coordinator",
        "O grupo precisa de ao menos uma pessoa na coordenação.",
    )
}

async fn list(State(state): State<AppState>, caller: CallerId) -> Response {
    let sql = format!("{GROUP_SELECT} ORDER BY g.name LIMIT 500");
    let rows: Result<Vec<GroupRow>, _> = sqlx::query_as(&sql)
        .bind(caller.org.as_uuid())
        .bind(caller.citizen.as_uuid())
        .fetch_all(&state.db)
        .await;
    match rows {
        Ok(rows) => (StatusCode::OK, Json(ApiResponse::ok(rows))).into_response(),
        Err(err) => storage(&err),
    }
}

async fn detail(
    State(state): State<AppState>,
    caller: CallerId,
    Path(slug): Path<String>,
) -> Response {
    let sql = format!("{GROUP_SELECT} AND g.slug = $3");
    let row: Result<Option<GroupRow>, _> = sqlx::query_as(&sql)
        .bind(caller.org.as_uuid())
        .bind(caller.citizen.as_uuid())
        .bind(slug)
        .fetch_optional(&state.db)
        .await;
    match row {
        Ok(Some(mut group)) => {
            // The forum's address only matters to those who can read it.
            if group.my_status.as_deref() != Some("active") {
                group.forum_path = None;
            }
            (StatusCode::OK, Json(ApiResponse::ok(group))).into_response()
        }
        Ok(None) => not_found(),
        Err(err) => storage(&err),
    }
}

fn valid_slug(slug: &str) -> bool {
    (3..=61).contains(&slug.len())
        && !slug.starts_with('-')
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn valid_entry(entry: &str) -> bool {
    matches!(entry, "open" | "approval")
}

fn valid_name(name: &str) -> bool {
    !name.trim().is_empty() && name.trim().chars().count() <= 120
}

#[derive(Debug, Deserialize)]
struct CreateGroup {
    slug: String,
    name: String,
    #[serde(default)]
    description: String,
    entry: Option<String>,
}

/// `POST /groups` — org admins only for now; the creator becomes the first coordinator.
async fn create(
    State(state): State<AppState>,
    caller: CallerId,
    Json(req): Json<CreateGroup>,
) -> Response {
    match is_org_admin(&state.db, caller).await {
        Ok(true) => {}
        Ok(false) => {
            return fail(
                StatusCode::FORBIDDEN,
                "forbidden",
                "Só a administração cria grupos.",
            )
        }
        Err(err) => return storage(&err),
    }
    let slug = req.slug.trim().to_lowercase();
    let entry = req.entry.as_deref().unwrap_or("approval");
    if !valid_slug(&slug) || !valid_name(&req.name) || !valid_entry(entry) {
        return invalid(
            "Use um endereço com letras minúsculas, números e hífens (mínimo 3) e um nome de até 120 caracteres.",
        );
    }
    let id = Uuid::now_v7();
    let result: Result<bool, sqlx::Error> = async {
        let mut tx = state.db.begin().await?;
        let inserted = sqlx::query(
            r"INSERT INTO community_group (id, org_id, slug, name, description, entry, created_by)
              VALUES ($1, $2, $3, $4, $5, $6, $7)
              ON CONFLICT (org_id, slug) DO NOTHING",
        )
        .bind(id)
        .bind(caller.org.as_uuid())
        .bind(&slug)
        .bind(req.name.trim())
        .bind(req.description.trim())
        .bind(entry)
        .bind(caller.citizen.as_uuid())
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        if inserted {
            sqlx::query(
                "INSERT INTO community_group_member (group_id, citizen_id, role) VALUES ($1, $2, 'coordinator')",
            )
            .bind(id)
            .bind(caller.citizen.as_uuid())
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(inserted)
    }
    .await;
    match result {
        Ok(true) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(serde_json::json!({"id": id, "slug": slug}))),
        )
            .into_response(),
        Ok(false) => fail(
            StatusCode::CONFLICT,
            "conflict",
            "Já existe um grupo com esse endereço.",
        ),
        Err(err) => storage(&err),
    }
}

#[derive(Debug, Deserialize)]
struct UpdateGroup {
    name: Option<String>,
    description: Option<String>,
    entry: Option<String>,
}

async fn update(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateGroup>,
) -> Response {
    if let Err(denied) = managed(&state, caller, id).await {
        return denied;
    }
    if req.name.as_deref().is_some_and(|n| !valid_name(n))
        || req.entry.as_deref().is_some_and(|e| !valid_entry(e))
    {
        return invalid("Dados inválidos.");
    }
    let res = sqlx::query(
        r"UPDATE community_group SET
             name = COALESCE($2, name),
             description = COALESCE($3, description),
             entry = COALESCE($4, entry)
           WHERE id = $1",
    )
    .bind(id)
    .bind(req.name.as_deref().map(str::trim))
    .bind(req.description.as_deref().map(str::trim))
    .bind(req.entry)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => ok_flag(),
        Err(err) => storage(&err),
    }
}

/// `DELETE /groups/{id}` — archive the group and its forum.
async fn archive(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<Uuid>,
) -> Response {
    let group = match managed(&state, caller, id).await {
        Ok(group) => group,
        Err(denied) => return denied,
    };
    let result: Result<(), sqlx::Error> = async {
        let mut tx = state.db.begin().await?;
        sqlx::query("UPDATE community_group SET archived_at = now() WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        if let Some(forum) = group.forum_id {
            sqlx::query("UPDATE forum SET hidden_at = now() WHERE id = $1 AND hidden_at IS NULL")
                .bind(forum)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await
    }
    .await;
    match result {
        Ok(()) => ok_flag(),
        Err(err) => storage(&err),
    }
}

/// `POST /groups/{id}/join` — an open group admits right away; otherwise coordinators decide.
async fn join(State(state): State<AppState>, caller: CallerId, Path(id): Path<Uuid>) -> Response {
    if let Err(err) = state
        .authz
        .require(caller.org, caller.citizen, VerificationLevel::Email)
        .await
    {
        tracing::debug!(?err, "groups: join below e-mail level");
        return fail(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Confirme seu e-mail para entrar em grupos.",
        );
    }
    let group = match standing(&state.db, caller, id).await {
        Ok(Some(group)) => group,
        Ok(None) => return not_found(),
        Err(err) => return storage(&err),
    };
    let status = if group.entry == "open" {
        "active"
    } else {
        "pending"
    };
    let citizen = caller.citizen.as_uuid();
    let result: Result<(), sqlx::Error> = async {
        let mut tx = state.db.begin().await?;
        let inserted = sqlx::query(
            r"INSERT INTO community_group_member (group_id, citizen_id, status)
              VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
        )
        .bind(id)
        .bind(citizen)
        .bind(status)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        if inserted && status == "active" {
            sync_forum(&mut tx, group.forum_id, citizen, true).await?;
        }
        tx.commit().await
    }
    .await;
    match result {
        Ok(()) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({"status": status}))),
        )
            .into_response(),
        Err(err) => storage(&err),
    }
}

/// Remove a person from the group and its forum, unless that leaves it without coordination.
/// `Ok(false)` means they were the last coordinator.
async fn remove_member(
    db: &PgPool,
    id: Uuid,
    forum: Option<Uuid>,
    citizen: Uuid,
) -> Result<bool, sqlx::Error> {
    let mut tx = db.begin().await?;
    lock_group(&mut tx, id).await?;
    if is_last_coordinator(&mut tx, id, citizen).await? {
        return Ok(false);
    }
    sqlx::query("DELETE FROM community_group_member WHERE group_id = $1 AND citizen_id = $2")
        .bind(id)
        .bind(citizen)
        .execute(&mut *tx)
        .await?;
    sync_forum(&mut tx, forum, citizen, false).await?;
    tx.commit().await?;
    Ok(true)
}

async fn leave(State(state): State<AppState>, caller: CallerId, Path(id): Path<Uuid>) -> Response {
    let group = match standing(&state.db, caller, id).await {
        Ok(Some(group)) => group,
        Ok(None) => return not_found(),
        Err(err) => return storage(&err),
    };
    match remove_member(&state.db, id, group.forum_id, caller.citizen.as_uuid()).await {
        Ok(true) => ok_flag(),
        Ok(false) => last_coordinator(),
        Err(err) => storage(&err),
    }
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct MemberRow {
    citizen_id: Uuid,
    handle: Option<String>,
    display_name: Option<String>,
    role: String,
    status: String,
}

/// `GET /groups/{id}/members` — active members see each other; managers also see requests.
async fn members(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<Uuid>,
) -> Response {
    let group = match standing(&state.db, caller, id).await {
        Ok(Some(group)) => group,
        Ok(None) => return not_found(),
        Err(err) => return storage(&err),
    };
    if !group.active_member() && !group.admin {
        return fail(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Só quem participa do grupo vê quem faz parte dele.",
        );
    }
    let rows: Result<Vec<MemberRow>, _> = sqlx::query_as(
        r"SELECT m.citizen_id, c.handle, c.display_name, m.role, m.status
            FROM community_group_member m JOIN citizen c ON c.id = m.citizen_id
           WHERE m.group_id = $1 AND c.deleted_at IS NULL
             AND (m.status = 'active' OR $2)
           ORDER BY m.status, m.role, c.display_name NULLS LAST",
    )
    .bind(id)
    .bind(group.manager())
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => (StatusCode::OK, Json(ApiResponse::ok(rows))).into_response(),
        Err(err) => storage(&err),
    }
}

#[derive(Debug, Deserialize)]
struct AddMember {
    citizen_id: Uuid,
}

/// `POST /groups/{id}/members` — the id comes from the person search below.
async fn member_add(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<Uuid>,
    Json(req): Json<AddMember>,
) -> Response {
    let group = match managed(&state, caller, id).await {
        Ok(group) => group,
        Err(denied) => return denied,
    };
    let result: Result<bool, sqlx::Error> = async {
        let mut tx = state.db.begin().await?;
        let in_org: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM citizen WHERE id = $1 AND org_id = $2 AND deleted_at IS NULL)",
        )
        .bind(req.citizen_id)
        .bind(caller.org.as_uuid())
        .fetch_one(&mut *tx)
        .await?;
        if !in_org {
            return Ok(false);
        }
        sqlx::query(
            r"INSERT INTO community_group_member (group_id, citizen_id, status)
              VALUES ($1, $2, 'active')
              ON CONFLICT (group_id, citizen_id) DO UPDATE SET status = 'active'",
        )
        .bind(id)
        .bind(req.citizen_id)
        .execute(&mut *tx)
        .await?;
        sync_forum(&mut tx, group.forum_id, req.citizen_id, true).await?;
        tx.commit().await?;
        Ok(true)
    }
    .await;
    match result {
        Ok(true) => ok_flag(),
        Ok(false) => fail(StatusCode::NOT_FOUND, "not_found", "Pessoa não encontrada."),
        Err(err) => storage(&err),
    }
}

#[derive(Debug, Deserialize)]
struct UpdateMember {
    /// `active` approves a pending request.
    status: Option<String>,
    role: Option<String>,
}

async fn member_update(
    State(state): State<AppState>,
    caller: CallerId,
    Path((id, citizen)): Path<(Uuid, Uuid)>,
    Json(req): Json<UpdateMember>,
) -> Response {
    let group = match managed(&state, caller, id).await {
        Ok(group) => group,
        Err(denied) => return denied,
    };
    if req.status.as_deref().is_some_and(|s| s != "active")
        || req
            .role
            .as_deref()
            .is_some_and(|r| !matches!(r, "coordinator" | "member"))
    {
        return invalid("Dados inválidos.");
    }
    let result: Result<Option<bool>, sqlx::Error> = async {
        let mut tx = state.db.begin().await?;
        lock_group(&mut tx, id).await?;
        if req.role.as_deref() == Some("member")
            && is_last_coordinator(&mut tx, id, citizen).await?
        {
            return Ok(Some(false));
        }
        let changed = sqlx::query(
            r"UPDATE community_group_member
                 SET status = COALESCE($3, status), role = COALESCE($4, role)
               WHERE group_id = $1 AND citizen_id = $2",
        )
        .bind(id)
        .bind(citizen)
        .bind(req.status.as_deref())
        .bind(req.role.as_deref())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if changed == 0 {
            return Ok(None);
        }
        if req.status.is_some() {
            sync_forum(&mut tx, group.forum_id, citizen, true).await?;
        }
        tx.commit().await?;
        Ok(Some(true))
    }
    .await;
    match result {
        Ok(Some(true)) => ok_flag(),
        Ok(Some(false)) => last_coordinator(),
        Ok(None) => fail(StatusCode::NOT_FOUND, "not_found", "Pessoa não encontrada."),
        Err(err) => storage(&err),
    }
}

async fn member_remove(
    State(state): State<AppState>,
    caller: CallerId,
    Path((id, citizen)): Path<(Uuid, Uuid)>,
) -> Response {
    let group = match managed(&state, caller, id).await {
        Ok(group) => group,
        Err(denied) => return denied,
    };
    match remove_member(&state.db, id, group.forum_id, citizen).await {
        Ok(true) => ok_flag(),
        Ok(false) => last_coordinator(),
        Err(err) => storage(&err),
    }
}

#[derive(Debug, Deserialize)]
struct PeopleQuery {
    q: Option<String>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct PersonRow {
    citizen_id: Uuid,
    handle: Option<String>,
    display_name: Option<String>,
}

/// Escape `LIKE` wildcards so a search can't enumerate everyone with `%`.
fn like_prefix(query: &str) -> String {
    let escaped = query
        .to_lowercase()
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("{escaped}%")
}

/// `GET /groups/{id}/people?q=` — for adding members: names and handles only, never e-mail, and
/// at least 3 characters, so it can't be used to list the whole community.
async fn people(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<Uuid>,
    Query(p): Query<PeopleQuery>,
) -> Response {
    if let Err(denied) = managed(&state, caller, id).await {
        return denied;
    }
    let q = p.q.unwrap_or_default();
    let q = q.trim().trim_start_matches('@');
    if q.chars().count() < 3 {
        return (
            StatusCode::OK,
            Json(ApiResponse::ok(Vec::<PersonRow>::new())),
        )
            .into_response();
    }
    let rows: Result<Vec<PersonRow>, _> = sqlx::query_as(
        r"SELECT c.id AS citizen_id, c.handle, c.display_name
            FROM citizen c
           WHERE c.org_id = $1 AND c.deleted_at IS NULL AND c.suspended_at IS NULL
             AND (lower(c.handle) LIKE $2 OR lower(c.display_name) LIKE $2)
             AND NOT EXISTS (SELECT 1 FROM community_group_member m
                              WHERE m.group_id = $3 AND m.citizen_id = c.id AND m.status = 'active')
           ORDER BY c.display_name NULLS LAST, c.handle
           LIMIT 10",
    )
    .bind(caller.org.as_uuid())
    .bind(like_prefix(q))
    .bind(id)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => (StatusCode::OK, Json(ApiResponse::ok(rows))).into_response(),
        Err(err) => storage(&err),
    }
}

/// `POST /groups/{id}/forum` — create the group's private forum, with current members on its list.
async fn enable_forum(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<Uuid>,
) -> Response {
    let group = match managed(&state, caller, id).await {
        Ok(group) => group,
        Err(denied) => return denied,
    };
    if group.forum_id.is_some() {
        return fail(StatusCode::CONFLICT, "conflict", "O grupo já tem fórum.");
    }
    let forum = Uuid::now_v7();
    let result: Result<Option<String>, sqlx::Error> = async {
        let mut tx = state.db.begin().await?;
        lock_group(&mut tx, id).await?;
        let (slug, name): (String, String) =
            sqlx::query_as("SELECT slug, name FROM community_group WHERE id = $1")
                .bind(id)
                .fetch_one(&mut *tx)
                .await?;
        let path = format!("grupo-{slug}");
        let inserted = sqlx::query(
            r"INSERT INTO forum (id, org_id, slug, full_path, name, description, kind, federated,
                                 visibility, created_by)
              VALUES ($1, $2, $3, $3, $4, 'Fórum privado do grupo.', 'comunitario', false, 'list', $5)
              ON CONFLICT DO NOTHING",
        )
        .bind(forum)
        .bind(caller.org.as_uuid())
        .bind(&path)
        .bind(&name)
        .bind(caller.citizen.as_uuid())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if inserted == 0 {
            return Ok(None);
        }
        sqlx::query(
            r"INSERT INTO forum_member (forum_id, citizen_id)
              SELECT $1, citizen_id FROM community_group_member
               WHERE group_id = $2 AND status = 'active'",
        )
        .bind(forum)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE community_group SET forum_id = $2 WHERE id = $1")
            .bind(id)
            .bind(forum)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Some(path))
    }
    .await;
    match result {
        Ok(Some(path)) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(serde_json::json!({"forum_path": path}))),
        )
            .into_response(),
        Ok(None) => fail(
            StatusCode::CONFLICT,
            "conflict",
            "Já existe um fórum com o endereço deste grupo.",
        ),
        Err(err) => storage(&err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_wildcards_are_literal() {
        assert_eq!(like_prefix("ana"), "ana%");
        assert_eq!(like_prefix("%_%"), "\\%\\_\\%%");
    }

    #[test]
    fn slugs_match_the_database_rule() {
        assert!(valid_slug("moderacao"));
        for bad in ["ab", "-abc", "Abc", "a b c"] {
            assert!(!valid_slug(bad), "{bad}");
        }
    }
}

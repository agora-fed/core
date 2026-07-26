//! `/admin/roles` — CRUD de papéis configuráveis + atribuição + catálogo de permissões (R4).
//!
//! Gated por `roles.manage` (via [`crate::authz_ext::require_permission`]) MAIS a hierarquia do
//! Mastodon: você só cria/edita/apaga papel de `position` MENOR que a sua, e só concede papel
//! abaixo de você. `administrator` bypassa a hierarquia. A matriz de checkboxes do front é
//! montada a partir de `GET /admin/permission-catalog`, que vem dos manifestos (R0.1).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, put};
use axum::{Json, Router};
use dsoc_api_contract::ApiResponse;
use dsoc_app::{AppState, CallerId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::authz_ext::require_permission;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/admin/permission-catalog", get(permission_catalog))
        .route("/admin/roles", get(list_roles).post(create_role))
        .route("/admin/roles/{id}", put(update_role).delete(delete_role))
        .route(
            "/admin/roles/{id}/members",
            get(list_members).post(add_member),
        )
        .route("/admin/roles/{id}/members/{cid}", delete(remove_member))
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

// ---------------------------------------------------------------------------
// Catálogo de permissões (dos manifestos) — alimenta a matriz de checkboxes.
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct PermissionDto {
    key: String,
    label: String,
    category: String,
    category_label: String,
}

async fn permission_catalog(State(state): State<AppState>, caller: CallerId) -> Response {
    if let Err(r) = require_permission(&state, caller, "roles.manage").await {
        return r;
    }
    let items: Vec<PermissionDto> = crate::module_catalog::permission_catalog()
        .into_iter()
        .map(|p| PermissionDto {
            key: p.key.to_owned(),
            label: p.label.to_owned(),
            category: p.category.slug().to_owned(),
            category_label: p.category.label().to_owned(),
        })
        .collect();
    (StatusCode::OK, Json(ApiResponse::ok(items))).into_response()
}

// ---------------------------------------------------------------------------
// Hierarquia
// ---------------------------------------------------------------------------

/// A maior `position` que o caller detém (papéis bindados + Base 0). `administrator` → i32::MAX.
async fn caller_max_position(state: &AppState, caller: CallerId) -> Result<i32, Response> {
    let svc = dsoc_admin::AdminService::from_state(state);
    let perms = svc
        .permissions_for(caller.org, caller.citizen)
        .await
        .map_err(|_| storage_error())?;
    if perms.is_administrator() {
        return Ok(i32::MAX);
    }
    let max: Option<i32> = sqlx::query_scalar(
        r"SELECT max(ur.position) FROM citizen_role_binding b
            JOIN user_role ur ON ur.id = b.role_id
           WHERE b.org_id = $1 AND b.citizen_id = $2",
    )
    .bind(caller.org.as_uuid())
    .bind(caller.citizen.as_uuid())
    .fetch_one(&state.db)
    .await
    .map_err(|_| storage_error())?;
    Ok(max.unwrap_or(0))
}

// ---------------------------------------------------------------------------
// CRUD de papéis
// ---------------------------------------------------------------------------

#[derive(Serialize, sqlx::FromRow)]
struct RoleDto {
    id: Uuid,
    name: String,
    color: Option<String>,
    position: i32,
    permissions: Vec<String>,
    highlighted: bool,
}

async fn list_roles(State(state): State<AppState>, caller: CallerId) -> Response {
    if let Err(r) = require_permission(&state, caller, "roles.manage").await {
        return r;
    }
    let rows = sqlx::query_as::<_, RoleDto>(
        "SELECT id, name, color, position, permissions, highlighted \
           FROM user_role WHERE org_id = $1 ORDER BY position DESC, name",
    )
    .bind(caller.org.as_uuid())
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(list) => (StatusCode::OK, Json(ApiResponse::ok(list))).into_response(),
        Err(err) => {
            tracing::warn!(?err, "list_roles");
            storage_error()
        }
    }
}

#[derive(Deserialize)]
struct RoleBody {
    name: String,
    #[serde(default)]
    color: Option<String>,
    position: i32,
    #[serde(default)]
    permissions: Vec<String>,
    #[serde(default)]
    highlighted: bool,
}

/// Valida chaves contra o catálogo (não deixa gravar chave inexistente) e normaliza.
fn validate_permissions(input: &[String]) -> Result<Vec<String>, Response> {
    let catalog: std::collections::BTreeSet<String> = crate::module_catalog::permission_catalog()
        .into_iter()
        .map(|p| p.key.to_owned())
        .collect();
    let mut out = Vec::new();
    for k in input {
        let k = k.trim();
        if k.is_empty() {
            continue;
        }
        if !catalog.contains(k) {
            return Err(fail(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_permission",
                "Permissão desconhecida no catálogo.",
            ));
        }
        if !out.iter().any(|e| e == k) {
            out.push(k.to_owned());
        }
    }
    Ok(out)
}

async fn create_role(
    State(state): State<AppState>,
    caller: CallerId,
    Json(body): Json<RoleBody>,
) -> Response {
    if let Err(r) = require_permission(&state, caller, "roles.manage").await {
        return r;
    }
    let max = match caller_max_position(&state, caller).await {
        Ok(m) => m,
        Err(r) => return r,
    };
    if body.position >= max {
        return fail(
            StatusCode::FORBIDDEN,
            "hierarchy",
            "Você não pode criar um papel com posição igual ou acima da sua.",
        );
    }
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return fail(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_name",
            "Nome inválido (1–60).",
        );
    }
    let perms = match validate_permissions(&body.permissions) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let res = sqlx::query(
        "INSERT INTO user_role (id, org_id, name, color, position, permissions, highlighted) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(Uuid::now_v7())
    .bind(caller.org.as_uuid())
    .bind(name)
    .bind(body.color.as_deref())
    .bind(body.position)
    .bind(&perms)
    .bind(body.highlighted)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(serde_json::json!({"ok": true}))),
        )
            .into_response(),
        Err(e) if e.as_database_error().and_then(|d| d.code()).as_deref() == Some("23505") => fail(
            StatusCode::CONFLICT,
            "conflict",
            "Já existe um papel com esse nome.",
        ),
        Err(err) => {
            tracing::warn!(?err, "create_role");
            storage_error()
        }
    }
}

async fn update_role(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<Uuid>,
    Json(body): Json<RoleBody>,
) -> Response {
    if let Err(r) = require_permission(&state, caller, "roles.manage").await {
        return r;
    }
    let max = match caller_max_position(&state, caller).await {
        Ok(m) => m,
        Err(r) => return r,
    };
    let current: Option<i32> =
        sqlx::query_scalar("SELECT position FROM user_role WHERE id = $1 AND org_id = $2")
            .bind(id)
            .bind(caller.org.as_uuid())
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    let Some(current) = current else {
        return fail(StatusCode::NOT_FOUND, "not_found", "Papel não encontrado.");
    };
    if current >= max || body.position >= max {
        return fail(
            StatusCode::FORBIDDEN,
            "hierarchy",
            "Fora do seu alcance de hierarquia.",
        );
    }
    let perms = match validate_permissions(&body.permissions) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let res = sqlx::query(
        "UPDATE user_role SET name = $3, color = $4, position = $5, permissions = $6, \
             highlighted = $7, updated_at = now() WHERE id = $1 AND org_id = $2",
    )
    .bind(id)
    .bind(caller.org.as_uuid())
    .bind(body.name.trim())
    .bind(body.color.as_deref())
    .bind(body.position)
    .bind(&perms)
    .bind(body.highlighted)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({"ok": true}))),
        )
            .into_response(),
        Err(err) => {
            tracing::warn!(?err, "update_role");
            storage_error()
        }
    }
}

async fn delete_role(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<Uuid>,
) -> Response {
    if let Err(r) = require_permission(&state, caller, "roles.manage").await {
        return r;
    }
    let max = match caller_max_position(&state, caller).await {
        Ok(m) => m,
        Err(r) => return r,
    };
    let row: Option<(i32, String)> =
        sqlx::query_as("SELECT position, name FROM user_role WHERE id = $1 AND org_id = $2")
            .bind(id)
            .bind(caller.org.as_uuid())
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    let Some((position, name)) = row else {
        return fail(StatusCode::NOT_FOUND, "not_found", "Papel não encontrado.");
    };
    if position >= max {
        return fail(
            StatusCode::FORBIDDEN,
            "hierarchy",
            "Fora do seu alcance de hierarquia.",
        );
    }
    if name == "Base" {
        return fail(
            StatusCode::UNPROCESSABLE_ENTITY,
            "protected",
            "O papel Base não pode ser removido.",
        );
    }
    let res = sqlx::query("DELETE FROM user_role WHERE id = $1 AND org_id = $2")
        .bind(id)
        .bind(caller.org.as_uuid())
        .execute(&state.db)
        .await;
    match res {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({"ok": true}))),
        )
            .into_response(),
        Err(err) => {
            tracing::warn!(?err, "delete_role");
            storage_error()
        }
    }
}

// ---------------------------------------------------------------------------
// Atribuição
// ---------------------------------------------------------------------------

#[derive(Serialize, sqlx::FromRow)]
struct MemberDto {
    citizen_id: Uuid,
    handle: Option<String>,
    display_name: Option<String>,
}

async fn list_members(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<Uuid>,
) -> Response {
    if let Err(r) = require_permission(&state, caller, "roles.manage").await {
        return r;
    }
    let rows = sqlx::query_as::<_, MemberDto>(
        "SELECT c.id AS citizen_id, c.handle, c.display_name \
           FROM citizen_role_binding b JOIN citizen c ON c.id = b.citizen_id \
          WHERE b.role_id = $1 AND b.org_id = $2 ORDER BY c.handle",
    )
    .bind(id)
    .bind(caller.org.as_uuid())
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(list) => (StatusCode::OK, Json(ApiResponse::ok(list))).into_response(),
        Err(err) => {
            tracing::warn!(?err, "list_members");
            storage_error()
        }
    }
}

#[derive(Deserialize)]
struct AddMemberBody {
    handle: String,
}

async fn add_member(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<Uuid>,
    Json(body): Json<AddMemberBody>,
) -> Response {
    if let Err(r) = require_permission(&state, caller, "roles.manage").await {
        return r;
    }
    let max = match caller_max_position(&state, caller).await {
        Ok(m) => m,
        Err(r) => return r,
    };
    let position: Option<i32> =
        sqlx::query_scalar("SELECT position FROM user_role WHERE id = $1 AND org_id = $2")
            .bind(id)
            .bind(caller.org.as_uuid())
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    let Some(position) = position else {
        return fail(StatusCode::NOT_FOUND, "not_found", "Papel não encontrado.");
    };
    if position >= max {
        return fail(
            StatusCode::FORBIDDEN,
            "hierarchy",
            "Você não pode conceder um papel igual ou acima do seu.",
        );
    }
    let handle = body.handle.trim().trim_start_matches('@');
    let citizen: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM citizen WHERE org_id = $1 AND handle = $2")
            .bind(caller.org.as_uuid())
            .bind(handle)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    let Some(citizen) = citizen else {
        return fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "Cidadão não encontrado por esse handle.",
        );
    };
    let res = sqlx::query(
        "INSERT INTO citizen_role_binding (id, org_id, citizen_id, role_id, created_by, created_at) \
         VALUES ($1, $2, $3, $4, $5, now()) ON CONFLICT (org_id, citizen_id, role_id) DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(caller.org.as_uuid())
    .bind(citizen)
    .bind(id)
    .bind(caller.citizen.as_uuid())
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({"ok": true}))),
        )
            .into_response(),
        Err(err) => {
            tracing::warn!(?err, "add_member");
            storage_error()
        }
    }
}

async fn remove_member(
    State(state): State<AppState>,
    caller: CallerId,
    Path((id, cid)): Path<(Uuid, Uuid)>,
) -> Response {
    if let Err(r) = require_permission(&state, caller, "roles.manage").await {
        return r;
    }
    let max = match caller_max_position(&state, caller).await {
        Ok(m) => m,
        Err(r) => return r,
    };
    let position: Option<i32> =
        sqlx::query_scalar("SELECT position FROM user_role WHERE id = $1 AND org_id = $2")
            .bind(id)
            .bind(caller.org.as_uuid())
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    let Some(position) = position else {
        return fail(StatusCode::NOT_FOUND, "not_found", "Papel não encontrado.");
    };
    if position >= max {
        return fail(
            StatusCode::FORBIDDEN,
            "hierarchy",
            "Fora do seu alcance de hierarquia.",
        );
    }
    let res = sqlx::query(
        "DELETE FROM citizen_role_binding WHERE role_id = $1 AND citizen_id = $2 AND org_id = $3",
    )
    .bind(id)
    .bind(cid)
    .bind(caller.org.as_uuid())
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({"ok": true}))),
        )
            .into_response(),
        Err(err) => {
            tracing::warn!(?err, "remove_member");
            storage_error()
        }
    }
}

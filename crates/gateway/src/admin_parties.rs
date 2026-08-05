//! `/admin/parties` — party directories and party administrators (AGORA campaign layer, #58).
//!
//! Surfaces the party/directory/administrator model that already exists in the schema
//! (migration 0204: `party`, `party_directory`, `party_administrator`) so the platform can
//! assign **Party Administrators** (party-wide, `directory_id IS NULL`) and **Directory
//! Administrators** (scoped to a federal/estadual/municipal directory). These roles gate the
//! campaign features (broadcast, SMS) built on top.
//!
//! English API by ADR-0013 (AGORA framework). Gated by `party.manage` (platform admin bypasses
//! via `administrator`). Runtime queries — no sqlx cache.
//!
//! - `GET    /admin/parties`                          — parties + directory/admin counts.
//! - `GET    /admin/parties/{sigla}/directories`      — directories of a party.
//! - `POST   /admin/parties/{sigla}/directories`      — create a directory.
//! - `GET    /admin/parties/{sigla}/administrators`   — administrators of a party.
//! - `POST   /admin/parties/{sigla}/administrators`   — assign an administrator.
//! - `DELETE /admin/parties/{sigla}/administrators/{id}` — remove an administrator.

use axum::extract::{Json, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::Router;
use chrono::{DateTime, Utc};
use dsoc_admin::permissions::keys;
use dsoc_api_contract::ApiResponse;
use dsoc_app::{AppState, CallerId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::authz_ext::require_permission;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/admin/parties", get(list_parties))
        .route(
            "/admin/parties/{sigla}/directories",
            get(list_directories).post(create_directory),
        )
        .route(
            "/admin/parties/{sigla}/administrators",
            get(list_administrators).post(assign_administrator),
        )
        .route(
            "/admin/parties/{sigla}/administrators/{id}",
            delete(remove_administrator),
        )
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

/// Record in `admin_audit` (#71) — best-effort, it never fails the request.
async fn audit(
    db: &sqlx::PgPool,
    admin_id: Uuid,
    action: &str,
    target_citizen_id: Option<Uuid>,
    target_domain: &str,
    target_id: Option<Uuid>,
    detail: serde_json::Value,
) {
    if let Err(err) = sqlx::query(
        r"INSERT INTO admin_audit (id, admin_id, action, target_citizen_id, target_domain, target_id, detail)
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
    {
        tracing::warn!(?err, action, "admin_parties: audit falhou (best-effort)");
    }
}

// ---------------------------------------------------------------------------
// Parties
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct PartyDto {
    sigla: String,
    name: String,
    directory_count: i64,
    administrator_count: i64,
}

async fn list_parties(State(state): State<AppState>, caller: CallerId) -> Response {
    let org = caller.org.as_uuid();
    if let Err(r) = require_permission(&state, caller, keys::PARTY_MANAGE).await {
        return r;
    }
    let rows: Result<Vec<(String, String, i64, i64)>, sqlx::Error> = sqlx::query_as(
        r"SELECT p.sigla, p.name,
                 (SELECT count(*) FROM party_directory d
                    WHERE d.org_id = p.org_id AND d.party_sigla = p.sigla) AS directory_count,
                 (SELECT count(*) FROM party_administrator a
                    WHERE a.org_id = p.org_id AND a.party_sigla = p.sigla) AS administrator_count
            FROM party p
           WHERE p.org_id = $1
           ORDER BY p.sigla",
    )
    .bind(org)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(items) => {
            let dtos: Vec<PartyDto> = items
                .into_iter()
                .map(
                    |(sigla, name, directory_count, administrator_count)| PartyDto {
                        sigla,
                        name,
                        directory_count,
                        administrator_count,
                    },
                )
                .collect();
            (StatusCode::OK, Json(ApiResponse::ok(dtos))).into_response()
        }
        Err(err) => {
            tracing::error!(?err, "list_parties");
            storage_error()
        }
    }
}

// ---------------------------------------------------------------------------
// Directories
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct DirectoryDto {
    id: Uuid,
    esfera: String,
    uf: Option<String>,
    municipio: Option<String>,
    name: String,
    parent_directory_id: Option<Uuid>,
    created_at: DateTime<Utc>,
}

async fn list_directories(
    State(state): State<AppState>,
    caller: CallerId,
    Path(sigla): Path<String>,
) -> Response {
    let org = caller.org.as_uuid();
    if let Err(r) = require_permission(&state, caller, keys::PARTY_MANAGE).await {
        return r;
    }
    let rows: Result<
        Vec<(
            Uuid,
            String,
            Option<String>,
            Option<String>,
            String,
            Option<Uuid>,
            DateTime<Utc>,
        )>,
        sqlx::Error,
    > = sqlx::query_as(
        r"SELECT id, esfera, uf, municipio, name, parent_directory_id, created_at
                FROM party_directory
               WHERE org_id = $1 AND party_sigla = $2
               ORDER BY esfera, uf NULLS FIRST, municipio NULLS FIRST",
    )
    .bind(org)
    .bind(&sigla)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(items) => {
            let dtos: Vec<DirectoryDto> = items
                .into_iter()
                .map(
                    |(id, esfera, uf, municipio, name, parent_directory_id, created_at)| {
                        DirectoryDto {
                            id,
                            esfera,
                            uf,
                            municipio,
                            name,
                            parent_directory_id,
                            created_at,
                        }
                    },
                )
                .collect();
            (StatusCode::OK, Json(ApiResponse::ok(dtos))).into_response()
        }
        Err(err) => {
            tracing::error!(?err, "list_directories");
            storage_error()
        }
    }
}

#[derive(Deserialize)]
struct CreateDirectoryBody {
    esfera: String,
    #[serde(default)]
    uf: Option<String>,
    #[serde(default)]
    municipio: Option<String>,
    name: String,
    #[serde(default)]
    parent_directory_id: Option<Uuid>,
    /// The citizen responsible for the directory — either this field OR `responsavel_handle`.
    /// Becomes a `party_administrator` with role `admin` scoped to the directory, in the same
    /// transaction as the creation (a directory is never born orphaned).
    #[serde(default)]
    responsavel_citizen_id: Option<Uuid>,
    #[serde(default)]
    responsavel_handle: Option<String>,
}

async fn create_directory(
    State(state): State<AppState>,
    caller: CallerId,
    Path(sigla): Path<String>,
    Json(body): Json<CreateDirectoryBody>,
) -> Response {
    let org = caller.org.as_uuid();
    let admin_id = caller.citizen.as_uuid();
    if let Err(r) = require_permission(&state, caller, keys::DIRECTORY_MANAGE).await {
        return r;
    }
    let esfera = body.esfera.trim().to_lowercase();
    let uf = body
        .uf
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_uppercase);
    let municipio = body
        .municipio
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let name = body.name.trim().to_owned();
    if name.is_empty() {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_name",
            "Nome do diretório é obrigatório.",
        );
    }
    // Federative shape (mirrors 0204's CHECK): federal ⇒ no uf/municipio; state ⇒ uf,
    // no municipio; municipal ⇒ uf + municipio.
    let shape_ok = match esfera.as_str() {
        "federal" => uf.is_none() && municipio.is_none(),
        "estadual" => uf.is_some() && municipio.is_none(),
        "municipal" => uf.is_some() && municipio.is_some(),
        _ => {
            return fail(
                StatusCode::BAD_REQUEST,
                "invalid_esfera",
                "Esfera deve ser federal, estadual ou municipal.",
            )
        }
    };
    if !shape_ok {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_shape",
            "federal: sem UF/município; estadual: só UF; municipal: UF + município.",
        );
    }
    // Responsible citizen mandatory (a direct id or an @handle resolved here) — the same
    // route contract of 0.37.0 in dsoc-mandates::parties.
    let responsavel = if let Some(id) = body.responsavel_citizen_id {
        id
    } else if let Some(handle) = body
        .responsavel_handle
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let handle = handle.trim_start_matches('@');
        match sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM citizen WHERE handle = $1 AND org_id = $2",
        )
        .bind(handle)
        .bind(org)
        .fetch_optional(&state.db)
        .await
        {
            Ok(Some(id)) => id,
            Ok(None) => {
                return fail(
                    StatusCode::NOT_FOUND,
                    "responsavel_not_found",
                    "Responsável não encontrado por handle.",
                )
            }
            Err(err) => {
                tracing::error!(?err, "create_directory: resolve responsável");
                return storage_error();
            }
        }
    } else {
        return fail(
            StatusCode::BAD_REQUEST,
            "missing_responsavel",
            "Informe o responsável pelo diretório (responsavel_citizen_id ou responsavel_handle).",
        );
    };
    // Single transaction: directory + responsible citizen, or nothing.
    let created: Result<Uuid, sqlx::Error> = async {
        let mut tx = state.db.begin().await?;
        let id: Uuid = sqlx::query_scalar(
            r"INSERT INTO party_directory (org_id, party_sigla, esfera, uf, municipio, name, parent_directory_id)
              VALUES ($1, $2, $3, $4, $5, $6, $7)
              RETURNING id",
        )
        .bind(org)
        .bind(&sigla)
        .bind(&esfera)
        .bind(&uf)
        .bind(&municipio)
        .bind(&name)
        .bind(body.parent_directory_id)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            r"INSERT INTO party_administrator
                (org_id, party_sigla, directory_id, citizen_id, role, invited_by, accepted_at)
              VALUES ($1, $2, $3, $4, 'admin', $5, now())",
        )
        .bind(org)
        .bind(&sigla)
        .bind(id)
        .bind(responsavel)
        .bind(admin_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(id)
    }
    .await;
    match created {
        Ok(id) => {
            audit(
                &state.db,
                admin_id,
                "party.directory.create",
                Some(responsavel),
                "party_directory",
                Some(id),
                serde_json::json!({ "party": sigla, "esfera": esfera, "uf": uf, "municipio": municipio, "responsavel": responsavel }),
            )
            .await;
            (StatusCode::CREATED, Json(ApiResponse::ok(id))).into_response()
        }
        // Territorial index (0673): a double click/retry never duplicates the directory.
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => fail(
            StatusCode::CONFLICT,
            "directory_exists",
            "Já existe um diretório deste partido neste território.",
        ),
        Err(sqlx::Error::Database(e)) if e.is_foreign_key_violation() => {
            if e.constraint().is_some_and(|c| c.contains("citizen")) {
                fail(
                    StatusCode::NOT_FOUND,
                    "responsavel_not_found",
                    "Responsável não encontrado.",
                )
            } else {
                fail(
                    StatusCode::NOT_FOUND,
                    "party_not_found",
                    "Partido não encontrado.",
                )
            }
        }
        Err(err) => {
            tracing::error!(?err, "create_directory");
            storage_error()
        }
    }
}

// ---------------------------------------------------------------------------
// Administrators
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct AdministratorDto {
    id: Uuid,
    directory_id: Option<Uuid>,
    citizen_id: Uuid,
    handle: Option<String>,
    role: String,
    created_at: DateTime<Utc>,
}

async fn list_administrators(
    State(state): State<AppState>,
    caller: CallerId,
    Path(sigla): Path<String>,
) -> Response {
    let org = caller.org.as_uuid();
    if let Err(r) = require_permission(&state, caller, keys::PARTY_MANAGE).await {
        return r;
    }
    let rows: Result<
        Vec<(
            Uuid,
            Option<Uuid>,
            Uuid,
            Option<String>,
            String,
            DateTime<Utc>,
        )>,
        sqlx::Error,
    > = sqlx::query_as(
        r"SELECT a.id, a.directory_id, a.citizen_id, c.handle, a.role, a.created_at
                FROM party_administrator a
                LEFT JOIN citizen c ON c.id = a.citizen_id
               WHERE a.org_id = $1 AND a.party_sigla = $2
               ORDER BY a.created_at",
    )
    .bind(org)
    .bind(&sigla)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(items) => {
            let dtos: Vec<AdministratorDto> = items
                .into_iter()
                .map(
                    |(id, directory_id, citizen_id, handle, role, created_at)| AdministratorDto {
                        id,
                        directory_id,
                        citizen_id,
                        handle,
                        role,
                        created_at,
                    },
                )
                .collect();
            (StatusCode::OK, Json(ApiResponse::ok(dtos))).into_response()
        }
        Err(err) => {
            tracing::error!(?err, "list_administrators");
            storage_error()
        }
    }
}

#[derive(Deserialize)]
struct AssignAdministratorBody {
    /// The citizen either by id or by `@handle` (handle is resolved server-side).
    #[serde(default)]
    citizen_id: Option<Uuid>,
    #[serde(default)]
    handle: Option<String>,
    role: String,
    #[serde(default)]
    directory_id: Option<Uuid>,
}

async fn assign_administrator(
    State(state): State<AppState>,
    caller: CallerId,
    Path(sigla): Path<String>,
    Json(body): Json<AssignAdministratorBody>,
) -> Response {
    let org = caller.org.as_uuid();
    let invited_by = caller.citizen.as_uuid();
    if let Err(r) = require_permission(&state, caller, keys::PARTY_MANAGE).await {
        return r;
    }
    let role = body.role.trim().to_lowercase();
    if role != "admin" && role != "moderador" {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_role",
            "Papel deve ser admin ou moderador.",
        );
    }
    // Resolve the citizen: prefer an explicit id, else look the `@handle` up.
    let citizen_id = if let Some(id) = body.citizen_id {
        id
    } else if let Some(handle) = body
        .handle
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let handle = handle.trim_start_matches('@');
        match sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM citizen WHERE handle = $1 AND org_id = $2",
        )
        .bind(handle)
        .bind(org)
        .fetch_optional(&state.db)
        .await
        {
            Ok(Some(id)) => id,
            Ok(None) => {
                return fail(
                    StatusCode::NOT_FOUND,
                    "citizen_not_found",
                    "Cidadão não encontrado por handle.",
                )
            }
            Err(err) => {
                tracing::error!(?err, "assign_administrator: resolve handle");
                return storage_error();
            }
        }
    } else {
        return fail(
            StatusCode::BAD_REQUEST,
            "missing_citizen",
            "Informe o handle ou o id do cidadão.",
        );
    };
    let row: Result<(Uuid,), sqlx::Error> = sqlx::query_as(
        r"INSERT INTO party_administrator (org_id, party_sigla, directory_id, citizen_id, role, invited_by, accepted_at)
          VALUES ($1, $2, $3, $4, $5, $6, now())
          RETURNING id",
    )
    .bind(org)
    .bind(&sigla)
    .bind(body.directory_id)
    .bind(citizen_id)
    .bind(&role)
    .bind(invited_by)
    .fetch_one(&state.db)
    .await;
    match row {
        Ok((id,)) => {
            audit(
                &state.db,
                invited_by,
                "party.administrator.assign",
                Some(citizen_id),
                "party_administrator",
                Some(id),
                serde_json::json!({ "party": sigla, "role": role, "directory_id": body.directory_id }),
            )
            .await;
            (StatusCode::CREATED, Json(ApiResponse::ok(id))).into_response()
        }
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => fail(
            StatusCode::CONFLICT,
            "already_administrator",
            "Esta pessoa já é administradora neste escopo.",
        ),
        Err(sqlx::Error::Database(e)) if e.is_foreign_key_violation() => fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "Partido, diretório ou cidadão não encontrado.",
        ),
        Err(err) => {
            tracing::error!(?err, "assign_administrator");
            storage_error()
        }
    }
}

async fn remove_administrator(
    State(state): State<AppState>,
    caller: CallerId,
    Path((sigla, id)): Path<(String, Uuid)>,
) -> Response {
    let org = caller.org.as_uuid();
    let admin_id = caller.citizen.as_uuid();
    if let Err(r) = require_permission(&state, caller, keys::PARTY_MANAGE).await {
        return r;
    }
    let res = sqlx::query("DELETE FROM party_administrator WHERE id = $1 AND org_id = $2")
        .bind(id)
        .bind(org)
        .execute(&state.db)
        .await;
    match res {
        Ok(r) if r.rows_affected() == 0 => fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "Administrador não encontrado.",
        ),
        Ok(_) => {
            audit(
                &state.db,
                admin_id,
                "party.administrator.remove",
                None,
                "party_administrator",
                Some(id),
                serde_json::json!({ "party": sigla }),
            )
            .await;
            (StatusCode::OK, Json(ApiResponse::ok(()))).into_response()
        }
        Err(err) => {
            tracing::error!(?err, "remove_administrator");
            storage_error()
        }
    }
}

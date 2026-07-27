//! `/admin/parties/{sigla}/directories/{id}/contacts` — base própria de contatos do diretório
//! (ÁGORA F4, #61, migration 0657).
//!
//! O diretório **sobe** sua própria lista (controlador = ele; base legal declarada). Fica isolada
//! por diretório — **apagável em bloco** (LGPD). Na importação, verificamos contra a base central:
//! e-mail que casa com um cidadão liga `matched_citizen_id` e enriquece o domicílio. Dedupe por
//! (diretório, e-mail). Gating reusa [`crate::campaign_broadcast::authorized`]. English API,
//! runtime queries.
//!
//! - `POST   /admin/parties/{sigla}/directories/{id}/contacts/import` — importa uma lista.
//! - `GET    /admin/parties/{sigla}/directories/{id}/contacts`        — estatísticas da base.
//! - `DELETE /admin/parties/{sigla}/directories/{id}/contacts`        — apaga a base (LGPD).

use axum::extract::{Json, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use dsoc_api_contract::ApiResponse;
use dsoc_app::{AppState, CallerId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::campaign_broadcast::authorized;

const MAX_IMPORT: usize = 2000;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route(
            "/admin/parties/{sigla}/directories/{id}/contacts",
            get(stats).delete(clear),
        )
        .route(
            "/admin/parties/{sigla}/directories/{id}/contacts/import",
            post(import),
        )
        .with_state(state)
}

fn fail(status: StatusCode, code: &str, msg: &str) -> Response {
    (status, Json(ApiResponse::<()>::fail(code, msg))).into_response()
}
fn storage_error() -> Response {
    fail(StatusCode::INTERNAL_SERVER_ERROR, "storage_error", "Erro interno.")
}

async fn gate(state: &AppState, caller: &CallerId, sigla: &str, directory_id: Uuid) -> Result<(), Response> {
    match authorized(state, caller, sigla, directory_id).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(fail(StatusCode::FORBIDDEN, "http_403", "Você não administra este partido/diretório.")),
        Err(r) => Err(r),
    }
}

#[derive(Deserialize)]
struct ContactInput {
    email: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    phone: Option<String>,
}

#[derive(Deserialize)]
struct ImportBody {
    #[serde(default = "default_basis")]
    legal_basis: String,
    contacts: Vec<ContactInput>,
}
fn default_basis() -> String {
    "consent".to_owned()
}

#[derive(Serialize, Default)]
struct ImportResult {
    received: usize,
    inserted: usize,
    duplicates: usize,
    matched: usize,
    invalid: usize,
}

async fn import(
    State(state): State<AppState>,
    caller: CallerId,
    Path((sigla, directory_id)): Path<(String, Uuid)>,
    Json(body): Json<ImportBody>,
) -> Response {
    let org = caller.org.as_uuid();
    if let Err(r) = gate(&state, &caller, &sigla, directory_id).await {
        return r;
    }
    let legal_basis = match body.legal_basis.trim() {
        "consent" | "legitimate_interest" | "contract" => body.legal_basis.trim().to_owned(),
        _ => return fail(StatusCode::BAD_REQUEST, "invalid_legal_basis", "Base legal inválida."),
    };
    if body.contacts.len() > MAX_IMPORT {
        return fail(StatusCode::BAD_REQUEST, "too_many", "Máximo de 2000 contatos por importação.");
    }
    // O diretório precisa existir na org e ser deste partido.
    let exists: bool = match sqlx::query_scalar(
        r"SELECT EXISTS(SELECT 1 FROM party_directory WHERE id = $1 AND org_id = $2 AND party_sigla = $3)",
    )
    .bind(directory_id)
    .bind(org)
    .bind(&sigla)
    .fetch_one(&state.db)
    .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "contacts import: dir check");
            return storage_error();
        }
    };
    if !exists {
        return fail(StatusCode::NOT_FOUND, "directory_not_found", "Diretório não encontrado.");
    }

    let mut res = ImportResult {
        received: body.contacts.len(),
        ..Default::default()
    };
    for c in body.contacts {
        let email = c.email.trim().to_lowercase();
        if email.len() < 3 || !email.contains('@') {
            res.invalid += 1;
            continue;
        }
        let name = c.name.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let phone = c.phone.as_deref().map(str::trim).filter(|s| !s.is_empty());
        // Insere já verificando contra a base central (match por e-mail → citizen + domicílio).
        let row: Result<Option<(bool,)>, sqlx::Error> = sqlx::query_as(
            r"WITH m AS (
                  SELECT c.id, c.uf, c.municipio_ibge
                    FROM auth_credential ac
                    JOIN citizen c ON c.id = ac.citizen_id
                   WHERE lower(ac.email) = $3 AND c.org_id = $1
                   LIMIT 1)
              INSERT INTO campaign_contact
                  (org_id, directory_id, email, name, phone, legal_basis, matched_citizen_id, uf, municipio_ibge)
              SELECT $1, $2, $3, $4, $5, $6, m.id, m.uf, m.municipio_ibge
                FROM (SELECT 1) d LEFT JOIN m ON true
              ON CONFLICT (directory_id, lower(email)) DO NOTHING
              RETURNING (matched_citizen_id IS NOT NULL)",
        )
        .bind(org)
        .bind(directory_id)
        .bind(&email)
        .bind(name)
        .bind(phone)
        .bind(&legal_basis)
        .fetch_optional(&state.db)
        .await;
        match row {
            Ok(Some((matched,))) => {
                res.inserted += 1;
                if matched {
                    res.matched += 1;
                }
            }
            Ok(None) => res.duplicates += 1,
            Err(err) => {
                tracing::error!(?err, "contacts import: insert");
                return storage_error();
            }
        }
    }

    (StatusCode::OK, Json(ApiResponse::ok(res))).into_response()
}

#[derive(Serialize)]
struct ContactStats {
    total: i64,
    matched: i64,
}

async fn stats(
    State(state): State<AppState>,
    caller: CallerId,
    Path((sigla, directory_id)): Path<(String, Uuid)>,
) -> Response {
    if let Err(r) = gate(&state, &caller, &sigla, directory_id).await {
        return r;
    }
    let row: Result<(i64, i64), sqlx::Error> = sqlx::query_as(
        r"SELECT count(*), count(matched_citizen_id) FROM campaign_contact WHERE directory_id = $1",
    )
    .bind(directory_id)
    .fetch_one(&state.db)
    .await;
    match row {
        Ok((total, matched)) => {
            (StatusCode::OK, Json(ApiResponse::ok(ContactStats { total, matched }))).into_response()
        }
        Err(err) => {
            tracing::error!(?err, "contacts stats");
            storage_error()
        }
    }
}

async fn clear(
    State(state): State<AppState>,
    caller: CallerId,
    Path((sigla, directory_id)): Path<(String, Uuid)>,
) -> Response {
    if let Err(r) = gate(&state, &caller, &sigla, directory_id).await {
        return r;
    }
    let res = sqlx::query("DELETE FROM campaign_contact WHERE directory_id = $1")
        .bind(directory_id)
        .execute(&state.db)
        .await;
    match res {
        Ok(r) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "deleted": r.rows_affected() }))),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "contacts clear");
            storage_error()
        }
    }
}

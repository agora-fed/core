//! `/me/campaign-consent` — citizen campaign-communication consent (ÁGORA F2, #59, migration 0654).
//!
//! LGPD art. 11 (dados sensíveis): consent is **specific, opt-in, default OFF**, and revocable.
//! Four capillarity levels the citizen may grant (multiple grants add up):
//!   - `all_parties`  — any directory of any party
//!   - `party`        — every directory of one party (`party_sigla`)
//!   - `municipality` — every party in one municipality (`uf` + `municipio`)
//!   - `directory`    — one party's directory in one municipality (`party_sigla` + `uf` + `municipio`)
//!
//! Citizen-facing (`/me/*`, authenticated via `CallerId`). English API by ADR-0013. Runtime
//! queries — no sqlx cache. Reach resolution (which directory reaches whom) lives in F3.
//!
//! - `GET    /me/campaign-consent`      — my active grants.
//! - `POST   /me/campaign-consent`      — grant a consent.
//! - `DELETE /me/campaign-consent/{id}` — revoke a grant.

use axum::extract::{Json, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::Router;
use chrono::{DateTime, Utc};
use dsoc_api_contract::ApiResponse;
use dsoc_app::{AppState, CallerId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route(
            "/me/campaign-consent",
            get(list_consent).post(grant_consent),
        )
        .route("/me/campaign-consent/{id}", delete(revoke_consent))
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

#[derive(Serialize)]
struct ConsentDto {
    id: Uuid,
    scope: String,
    party_sigla: Option<String>,
    uf: Option<String>,
    municipio: Option<String>,
    granted_at: DateTime<Utc>,
}

async fn list_consent(State(state): State<AppState>, caller: CallerId) -> Response {
    let (org, citizen) = (caller.org.as_uuid(), caller.citizen.as_uuid());
    let rows: Result<
        Vec<(
            Uuid,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            DateTime<Utc>,
        )>,
        sqlx::Error,
    > = sqlx::query_as(
        r"SELECT id, scope, party_sigla, uf, municipio, granted_at
                FROM citizen_campaign_consent
               WHERE citizen_id = $1 AND org_id = $2 AND revoked_at IS NULL
               ORDER BY granted_at DESC",
    )
    .bind(citizen)
    .bind(org)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(items) => {
            let dtos: Vec<ConsentDto> = items
                .into_iter()
                .map(
                    |(id, scope, party_sigla, uf, municipio, granted_at)| ConsentDto {
                        id,
                        scope,
                        party_sigla,
                        uf,
                        municipio,
                        granted_at,
                    },
                )
                .collect();
            (StatusCode::OK, Json(ApiResponse::ok(dtos))).into_response()
        }
        Err(err) => {
            tracing::error!(?err, "list_consent");
            storage_error()
        }
    }
}

#[derive(Deserialize)]
struct GrantBody {
    scope: String,
    #[serde(default)]
    party_sigla: Option<String>,
    #[serde(default)]
    uf: Option<String>,
    #[serde(default)]
    municipio: Option<String>,
}

async fn grant_consent(
    State(state): State<AppState>,
    caller: CallerId,
    Json(body): Json<GrantBody>,
) -> Response {
    let (org, citizen) = (caller.org.as_uuid(), caller.citizen.as_uuid());
    let scope = body.scope.trim().to_lowercase();
    let party = body
        .party_sigla
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
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

    // Shape por escopo (espelha o CHECK da 0654).
    let shape_ok = match scope.as_str() {
        "all_parties" => party.is_none() && uf.is_none() && municipio.is_none(),
        "party" => party.is_some() && uf.is_none() && municipio.is_none(),
        "municipality" => party.is_none() && uf.is_some() && municipio.is_some(),
        "directory" => party.is_some() && uf.is_some() && municipio.is_some(),
        _ => {
            return fail(
                StatusCode::BAD_REQUEST,
                "invalid_scope",
                "Escopo deve ser all_parties, party, municipality ou directory.",
            )
        }
    };
    if !shape_ok {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_shape",
            "Campos party_sigla/uf/municipio não batem com o escopo escolhido.",
        );
    }

    let row: Result<(Uuid,), sqlx::Error> = sqlx::query_as(
        r"INSERT INTO citizen_campaign_consent (org_id, citizen_id, scope, party_sigla, uf, municipio)
          VALUES ($1, $2, $3, $4, $5, $6)
          RETURNING id",
    )
    .bind(org)
    .bind(citizen)
    .bind(&scope)
    .bind(&party)
    .bind(&uf)
    .bind(&municipio)
    .fetch_one(&state.db)
    .await;
    match row {
        Ok((id,)) => (StatusCode::CREATED, Json(ApiResponse::ok(id))).into_response(),
        Err(err) => {
            tracing::error!(?err, "grant_consent");
            storage_error()
        }
    }
}

async fn revoke_consent(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<Uuid>,
) -> Response {
    let citizen = caller.citizen.as_uuid();
    let res = sqlx::query(
        r"UPDATE citizen_campaign_consent
             SET revoked_at = now()
           WHERE id = $1 AND citizen_id = $2 AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(citizen)
    .execute(&state.db)
    .await;
    match res {
        Ok(r) if r.rows_affected() == 0 => fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "Consentimento não encontrado.",
        ),
        Ok(_) => (StatusCode::OK, Json(ApiResponse::ok(()))).into_response(),
        Err(err) => {
            tracing::error!(?err, "revoke_consent");
            storage_error()
        }
    }
}

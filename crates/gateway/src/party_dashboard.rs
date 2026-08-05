//! `/admin/parties/{sigla}/dashboard` — the party/directory campaign panel (AGORA F7, #64).
//!
//! Aggregates, read-only, what the previous phases produced: size of the own contact base (F4), the
//! consent rate per level (F2), the history of broadcasts + micro-consultations (F3). Gating: platform
//! plataforma OU qualquer `party_administrator` do partido. English API, runtime queries.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use dsoc_admin::permissions::keys;
use dsoc_admin::AdminService;
use dsoc_api_contract::ApiResponse;
use dsoc_app::{AppState, CallerId};
use serde::Serialize;
use uuid::Uuid;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/admin/parties/{sigla}/dashboard", get(dashboard))
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

/// Admin de plataforma OU qualquer party_administrator do partido.
async fn party_authorized(
    state: &AppState,
    caller: &CallerId,
    sigla: &str,
) -> Result<bool, Response> {
    let org = caller.org.as_uuid();
    let citizen = caller.citizen.as_uuid();
    let perms = AdminService::from_state(state)
        .permissions_for(caller.org, caller.citizen)
        .await
        .map_err(|_| storage_error())?;
    if perms.is_administrator() || perms.can(keys::PARTY_MANAGE) {
        return Ok(true);
    }
    let is_admin: bool = sqlx::query_scalar(
        r"SELECT EXISTS(SELECT 1 FROM party_administrator WHERE org_id = $1 AND party_sigla = $2 AND citizen_id = $3)",
    )
    .bind(org)
    .bind(sigla)
    .bind(citizen)
    .fetch_one(&state.db)
    .await
    .map_err(|_| storage_error())?;
    Ok(is_admin)
}

#[derive(Serialize)]
struct ConsentBreakdown {
    all_parties: i64,
    this_party: i64,
    directory_this_party: i64,
    municipality_any: i64,
}

#[derive(Serialize)]
struct BroadcastRow {
    subject: String,
    recipients: i32,
    created_at: DateTime<Utc>,
    consultation_id: Option<Uuid>,
    uf: Option<String>,
    municipio: Option<String>,
}

#[derive(Serialize)]
struct Dashboard {
    directories_count: i64,
    administrators_count: i64,
    consent: ConsentBreakdown,
    own_contacts_total: i64,
    own_contacts_matched: i64,
    broadcasts_count: i64,
    broadcasts: Vec<BroadcastRow>,
}

async fn dashboard(
    State(state): State<AppState>,
    caller: CallerId,
    Path(sigla): Path<String>,
) -> Response {
    let org = caller.org.as_uuid();
    match party_authorized(&state, &caller, &sigla).await {
        Ok(true) => {}
        Ok(false) => {
            return fail(
                StatusCode::FORBIDDEN,
                "http_403",
                "Você não administra este partido.",
            )
        }
        Err(r) => return r,
    }

    // Counts of directories/administrators.
    let (directories_count, administrators_count): (i64, i64) = match sqlx::query_as(
        r"SELECT (SELECT count(*) FROM party_directory     WHERE org_id = $1 AND party_sigla = $2),
                 (SELECT count(*) FROM party_administrator WHERE org_id = $1 AND party_sigla = $2)",
    )
    .bind(org)
    .bind(&sigla)
    .fetch_one(&state.db)
    .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "dashboard: counts");
            return storage_error();
        }
    };

    // Active consent per level (F2).
    let (all_parties, this_party, directory_this_party, municipality_any): (i64, i64, i64, i64) =
        match sqlx::query_as(
            r"SELECT
                count(*) FILTER (WHERE scope = 'all_parties'),
                count(*) FILTER (WHERE scope = 'party'        AND party_sigla = $1),
                count(*) FILTER (WHERE scope = 'directory'    AND party_sigla = $1),
                count(*) FILTER (WHERE scope = 'municipality')
              FROM citizen_campaign_consent
             WHERE org_id = $2 AND revoked_at IS NULL",
        )
        .bind(&sigla)
        .bind(org)
        .fetch_one(&state.db)
        .await
        {
            Ok(v) => v,
            Err(err) => {
                tracing::error!(?err, "dashboard: consent");
                return storage_error();
            }
        };

    // Own contact base (F4) — total + matches in the central base, across the party's directories.
    let (own_total, own_matched): (i64, i64) = match sqlx::query_as(
        r"SELECT count(*), count(cc.matched_citizen_id)
            FROM campaign_contact cc
            JOIN party_directory d ON d.id = cc.directory_id
           WHERE d.org_id = $1 AND d.party_sigla = $2",
    )
    .bind(org)
    .bind(&sigla)
    .fetch_one(&state.db)
    .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "dashboard: contacts");
            return storage_error();
        }
    };

    // Broadcast history (F3) — the last 20 + the total.
    let broadcasts_count: i64 = sqlx::query_scalar(
        r"SELECT count(*) FROM campaign_broadcast WHERE org_id = $1 AND party_sigla = $2",
    )
    .bind(org)
    .bind(&sigla)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    let rows: Vec<(
        String,
        i32,
        DateTime<Utc>,
        Option<Uuid>,
        Option<String>,
        Option<String>,
    )> = match sqlx::query_as(
        r"SELECT b.subject, b.recipients, b.created_at, b.consultation_id, d.uf, d.municipio
                FROM campaign_broadcast b
                LEFT JOIN party_directory d ON d.id = b.directory_id
               WHERE b.org_id = $1 AND b.party_sigla = $2
               ORDER BY b.created_at DESC
               LIMIT 20",
    )
    .bind(org)
    .bind(&sigla)
    .fetch_all(&state.db)
    .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "dashboard: broadcasts");
            return storage_error();
        }
    };
    let broadcasts = rows
        .into_iter()
        .map(
            |(subject, recipients, created_at, consultation_id, uf, municipio)| BroadcastRow {
                subject,
                recipients,
                created_at,
                consultation_id,
                uf,
                municipio,
            },
        )
        .collect();

    (
        StatusCode::OK,
        Json(ApiResponse::ok(Dashboard {
            directories_count,
            administrators_count,
            consent: ConsentBreakdown {
                all_parties,
                this_party,
                directory_this_party,
                municipality_any,
            },
            own_contacts_total: own_total,
            own_contacts_matched: own_matched,
            broadcasts_count,
            broadcasts,
        })),
    )
        .into_response()
}

//! `/admin/parties/{sigla}/directories/{id}/sms-gateway` — config do SMSGateway do diretório
//! (INTERCOMS #69, migration 0660). Cada diretório cadastra seu **próprio** SMSGateway
//! (host + credenciais), **cifrado em repouso** (pgcrypto, chave em `INTERCOMS_CONFIG_KEY`).
//!
//! Gating reusa [`crate::campaign_broadcast::authorized`]. English API, runtime queries. O envio
//! que consome esta config é a fatia #69b (broadcast SMS).
//!
//! - `GET    …/sms-gateway` — {configured, url} (credenciais nunca retornam).
//! - `PUT    …/sms-gateway` — grava/atualiza {url, user?, pass?}.
//! - `DELETE …/sms-gateway` — remove.

use axum::extract::{Json, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use dsoc_api_contract::ApiResponse;
use dsoc_app::{AppState, CallerId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::campaign_broadcast::authorized;
use crate::intercoms::config_key;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route(
            "/admin/parties/{sigla}/directories/{id}/sms-gateway",
            get(get_config).put(set_config).delete(delete_config),
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

async fn gate(state: &AppState, caller: &CallerId, sigla: &str, dir: Uuid) -> Result<(), Response> {
    match authorized(state, caller, sigla, dir).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(fail(
            StatusCode::FORBIDDEN,
            "http_403",
            "Você não administra este diretório.",
        )),
        Err(r) => Err(r),
    }
}

#[derive(Serialize)]
struct ConfigStatus {
    configured: bool,
    url: Option<String>,
}

async fn get_config(
    State(state): State<AppState>,
    caller: CallerId,
    Path((sigla, dir)): Path<(String, Uuid)>,
) -> Response {
    if let Err(r) = gate(&state, &caller, &sigla, dir).await {
        return r;
    }
    let Some(key) = config_key() else {
        return (
            StatusCode::OK,
            Json(ApiResponse::ok(ConfigStatus {
                configured: false,
                url: None,
            })),
        )
            .into_response();
    };
    // Decifra e devolve SÓ a url (credenciais nunca voltam).
    let row: Result<Option<(String,)>, sqlx::Error> = sqlx::query_as(
        r"SELECT pgp_sym_decrypt(config, $2) FROM intercoms_provider_config
           WHERE directory_id = $1 AND channel = 'sms'",
    )
    .bind(dir)
    .bind(&key)
    .fetch_optional(&state.db)
    .await;
    match row {
        Ok(Some((json,))) => {
            let url = serde_json::from_str::<serde_json::Value>(&json)
                .ok()
                .and_then(|v| v.get("url").and_then(|u| u.as_str()).map(str::to_owned));
            (
                StatusCode::OK,
                Json(ApiResponse::ok(ConfigStatus {
                    configured: true,
                    url,
                })),
            )
                .into_response()
        }
        Ok(None) => (
            StatusCode::OK,
            Json(ApiResponse::ok(ConfigStatus {
                configured: false,
                url: None,
            })),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "sms-gateway get");
            storage_error()
        }
    }
}

#[derive(Deserialize)]
struct SetBody {
    url: String,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    pass: Option<String>,
}

async fn set_config(
    State(state): State<AppState>,
    caller: CallerId,
    Path((sigla, dir)): Path<(String, Uuid)>,
    Json(body): Json<SetBody>,
) -> Response {
    let org = caller.org.as_uuid();
    if let Err(r) = gate(&state, &caller, &sigla, dir).await {
        return r;
    }
    let Some(key) = config_key() else {
        return fail(
            StatusCode::SERVICE_UNAVAILABLE,
            "no_config_key",
            "Config de SMS ainda não habilitada nesta instância (falta INTERCOMS_CONFIG_KEY).",
        );
    };
    let url = body.url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_url",
            "URL do SMSGateway deve começar com http(s)://.",
        );
    }
    let json = serde_json::json!({
        "url": url,
        "user": body.user.as_deref().map(str::trim).filter(|s| !s.is_empty()),
        "pass": body.pass.as_deref().map(str::trim).filter(|s| !s.is_empty()),
    })
    .to_string();

    // Upsert por (diretório, canal): delete + insert numa tx.
    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(_) => return storage_error(),
    };
    if sqlx::query(
        "DELETE FROM intercoms_provider_config WHERE directory_id = $1 AND channel = 'sms'",
    )
    .bind(dir)
    .execute(&mut *tx)
    .await
    .is_err()
    {
        return storage_error();
    }
    if let Err(err) = sqlx::query(
        r"INSERT INTO intercoms_provider_config (org_id, directory_id, channel, provider, config)
          VALUES ($1, $2, 'sms', 'smsgateway', pgp_sym_encrypt($3, $4))",
    )
    .bind(org)
    .bind(dir)
    .bind(&json)
    .bind(&key)
    .execute(&mut *tx)
    .await
    {
        // FK do diretório inválido → 404.
        if let sqlx::Error::Database(e) = &err {
            if e.is_foreign_key_violation() {
                return fail(
                    StatusCode::NOT_FOUND,
                    "directory_not_found",
                    "Diretório não encontrado.",
                );
            }
        }
        tracing::error!(?err, "sms-gateway set");
        return storage_error();
    }
    if tx.commit().await.is_err() {
        return storage_error();
    }
    (
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({ "configured": true }))),
    )
        .into_response()
}

async fn delete_config(
    State(state): State<AppState>,
    caller: CallerId,
    Path((sigla, dir)): Path<(String, Uuid)>,
) -> Response {
    if let Err(r) = gate(&state, &caller, &sigla, dir).await {
        return r;
    }
    let res = sqlx::query(
        "DELETE FROM intercoms_provider_config WHERE directory_id = $1 AND channel = 'sms'",
    )
    .bind(dir)
    .execute(&state.db)
    .await;
    match res {
        Ok(r) => (
            StatusCode::OK,
            Json(ApiResponse::ok(
                serde_json::json!({ "deleted": r.rows_affected() }),
            )),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "sms-gateway delete");
            storage_error()
        }
    }
}

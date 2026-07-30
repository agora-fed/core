//! `/admin/parties/{sigla}/directories/{id}/broadcast-sms` — broadcast SMS consentido de campanha
//! (ÁGORA #69b, INTERCOMS/ADR-0016). Complementa o broadcast por e-mail (F3) com o canal SMS.
//!
//! Um diretório **municipal** dispara um SMS curto à sua base consentida que **verificou o
//! telefone**, usando o **SMSGateway do próprio diretório** (config cifrada #69a). A plataforma
//! resolve QUEM autorizou (consentimento 4-níveis 0654 × domicílio 0652) e envia **via INTERCOMS**
//! (`SmsGatewayProvider`) — nunca expondo a lista.
//!
//! **Rate-limit (regra do produto):** diretórios/candidatos podem enviar **1 SMS por semana**;
//! **somente o OWNER da plataforma** (administrador) envia sem limite. O cooldown de 24h dos
//! e-mails é independente — SMS custa dinheiro e é mais intrusivo, logo tem trava própria.
//!
//! Gating reusa [`crate::campaign_broadcast::authorized`]. English API por ADR-0013. Runtime queries.

use axum::extract::{Json, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use dsoc_admin::AdminService;
use dsoc_api_contract::ApiResponse;
use dsoc_app::{AppState, CallerId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::campaign_broadcast::authorized;
use crate::intercoms::{
    config_key, Channel, MessageSender, OutboundMessage, SmsConfig, SmsGatewayProvider,
};

/// Teto de destinatários por disparo (defesa; fase municipal é de baixo volume).
const MAX_RECIPIENTS: i64 = 5000;
/// Rate-limit de SMS para diretórios/candidatos (o OWNER da plataforma é ilimitado).
const SMS_WINDOW_DAYS: i64 = 7;
/// SMS é curto por natureza (custo + segmentação). Teto conservador (~2 segmentos).
const MAX_SMS_BODY: usize = 300;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route(
            "/admin/parties/{sigla}/directories/{id}/broadcast-sms",
            post(broadcast_sms),
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

#[derive(Deserialize)]
struct SmsBody {
    body: String,
}

#[derive(Serialize)]
struct SmsResult {
    recipients: i64,
    broadcast_id: Uuid,
}

/// Decifra a config de SMSGateway do diretório (JSON `{url,user,pass}`). `Ok(None)` = não configurado.
async fn load_sms_config(state: &AppState, dir: Uuid) -> Result<Option<SmsConfig>, Response> {
    let Some(key) = config_key() else {
        return Ok(None);
    };
    let row: Option<(String,)> = sqlx::query_as(
        r"SELECT pgp_sym_decrypt(config, $2) FROM intercoms_provider_config
           WHERE directory_id = $1 AND channel = 'sms'",
    )
    .bind(dir)
    .bind(&key)
    .fetch_optional(&state.db)
    .await
    .map_err(|err| {
        tracing::error!(?err, "broadcast-sms: load config");
        storage_error()
    })?;
    let Some((json,)) = row else {
        return Ok(None);
    };
    let v: serde_json::Value = serde_json::from_str(&json).map_err(|_| storage_error())?;
    let Some(url) = v.get("url").and_then(|u| u.as_str()).map(str::to_owned) else {
        return Ok(None);
    };
    Ok(Some(SmsConfig {
        url,
        user: v.get("user").and_then(|u| u.as_str()).map(str::to_owned),
        pass: v.get("pass").and_then(|u| u.as_str()).map(str::to_owned),
    }))
}

async fn broadcast_sms(
    State(state): State<AppState>,
    caller: CallerId,
    Path((sigla, directory_id)): Path<(String, Uuid)>,
    Json(body): Json<SmsBody>,
) -> Response {
    let org = caller.org.as_uuid();
    let sent_by = caller.citizen.as_uuid();

    // Gate: admin de plataforma OU party_administrator (nacional/deste diretório).
    match authorized(&state, &caller, &sigla, directory_id).await {
        Ok(true) => {}
        Ok(false) => {
            return fail(
                StatusCode::FORBIDDEN,
                "http_403",
                "Você não administra este partido/diretório.",
            )
        }
        Err(r) => return r,
    }

    // OWNER da plataforma (administrador) = ilimitado; demais = 1 SMS/semana por diretório.
    let is_owner = match AdminService::from_state(&state)
        .permissions_for(caller.org, caller.citizen)
        .await
    {
        Ok(p) => p.is_administrator(),
        Err(err) => {
            tracing::error!(?err, "broadcast-sms: perms");
            return storage_error();
        }
    };

    let msg_body = body.body.trim().to_owned();
    if msg_body.is_empty() || msg_body.chars().count() > MAX_SMS_BODY {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_body",
            "SMS de 1 a 300 caracteres.",
        );
    }

    // Diretório precisa existir na org, ser deste partido e ser MUNICIPAL (uf + município).
    let dir: Option<(Option<String>, Option<String>)> = match sqlx::query_as(
        r"SELECT uf, municipio FROM party_directory
           WHERE id = $1 AND org_id = $2 AND party_sigla = $3",
    )
    .bind(directory_id)
    .bind(org)
    .bind(&sigla)
    .fetch_optional(&state.db)
    .await
    {
        Ok(d) => d,
        Err(err) => {
            tracing::error!(?err, "broadcast-sms: load directory");
            return storage_error();
        }
    };
    let Some((uf, municipio)) = dir else {
        return fail(
            StatusCode::NOT_FOUND,
            "directory_not_found",
            "Diretório não encontrado.",
        );
    };
    let (Some(uf), Some(municipio)) = (uf, municipio) else {
        return fail(
            StatusCode::BAD_REQUEST,
            "not_municipal",
            "Por ora o broadcast é só para diretórios municipais.",
        );
    };

    // Config de SMSGateway do diretório (obrigatória para SMS).
    let sms_cfg = match load_sms_config(&state, directory_id).await {
        Ok(Some(c)) => c,
        Ok(None) => {
            return fail(
                StatusCode::BAD_REQUEST,
                "no_sms_gateway",
                "Configure o SMSGateway deste diretório antes de enviar SMS.",
            )
        }
        Err(r) => return r,
    };

    // Rate-limit: exceto OWNER, 1 SMS/semana por diretório.
    if !is_owner {
        let recent: bool = match sqlx::query_scalar(
            r"SELECT EXISTS(SELECT 1 FROM campaign_broadcast
                             WHERE directory_id = $1 AND channel = 'sms'
                               AND created_at > now() - ($2 || ' days')::interval)",
        )
        .bind(directory_id)
        .bind(SMS_WINDOW_DAYS.to_string())
        .fetch_one(&state.db)
        .await
        {
            Ok(v) => v,
            Err(err) => {
                tracing::error!(?err, "broadcast-sms: rate-limit check");
                return storage_error();
            }
        };
        if recent {
            return fail(
                StatusCode::TOO_MANY_REQUESTS,
                "sms_rate_limit",
                "Este diretório já enviou um SMS nos últimos 7 dias (limite de 1/semana).",
            );
        }
    }

    // Alcance: cidadãos que RESIDEM no município do diretório, com TELEFONE VERIFICADO, e que
    // consentiram num nível que cobre este diretório. Município case-insensitive (texto × IBGE).
    let phones: Vec<(String,)> = match sqlx::query_as(
        r"SELECT DISTINCT c.phone
            FROM citizen c
            JOIN municipio_ibge mi ON mi.codigo_ibge = c.municipio_ibge
           WHERE c.org_id = $1 AND c.uf = $2 AND upper(mi.nome) = upper($3)
             AND c.phone IS NOT NULL AND c.phone_verified_at IS NOT NULL
             AND EXISTS (
                 SELECT 1 FROM citizen_campaign_consent cc
                  WHERE cc.citizen_id = c.id AND cc.revoked_at IS NULL AND (
                      cc.scope = 'all_parties'
                   OR (cc.scope = 'party'        AND cc.party_sigla = $4)
                   OR (cc.scope = 'municipality' AND cc.uf = $2 AND upper(cc.municipio) = upper($3))
                   OR (cc.scope = 'directory'    AND cc.party_sigla = $4 AND cc.uf = $2 AND upper(cc.municipio) = upper($3))
                 ))
           LIMIT $5",
    )
    .bind(org)
    .bind(&uf)
    .bind(&municipio)
    .bind(&sigla)
    .bind(MAX_RECIPIENTS)
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(err) => {
            tracing::error!(?err, "broadcast-sms: resolve reach");
            return storage_error();
        }
    };
    let recipients = phones.len() as i64;

    // Registra o broadcast (canal SMS) ANTES de enviar — auditoria + base do rate-limit.
    let subject = format!("SMS {sigla}");
    let broadcast_id: Uuid = match sqlx::query_scalar(
        r"INSERT INTO campaign_broadcast
             (org_id, party_sigla, directory_id, sent_by, channel, subject, body, recipients)
          VALUES ($1, $2, $3, $4, 'sms', $5, $6, $7) RETURNING id",
    )
    .bind(org)
    .bind(&sigla)
    .bind(directory_id)
    .bind(sent_by)
    .bind(&subject)
    .bind(&msg_body)
    .bind(recipients)
    .fetch_one(&state.db)
    .await
    {
        Ok(id) => id,
        Err(err) => {
            tracing::error!(?err, "broadcast-sms: insert record");
            return storage_error();
        }
    };

    // Envio em background (best-effort) via SMSGateway do diretório — não trava a requisição.
    let sender = SmsGatewayProvider::new(sms_cfg);
    // Rodapé curto de opt-out (SMS é caro/segmentado → mínimo).
    let full = format!("{msg_body}\nSair: democracia.social.br/configuracoes#campanha");
    let list: Vec<String> = phones.into_iter().map(|(p,)| p).collect();
    tokio::spawn(async move {
        for to in list {
            let msg = OutboundMessage {
                channel: Channel::Sms,
                to: to.clone(),
                subject: String::new(),
                body: full.clone(),
            };
            if let Err(err) = sender.send(&msg).await {
                tracing::warn!(?err, "broadcast-sms: envio falhou (best-effort)");
            }
        }
    });

    (
        StatusCode::OK,
        Json(ApiResponse::ok(SmsResult {
            recipients,
            broadcast_id,
        })),
    )
        .into_response()
}

//! `/admin/parties/{sigla}/directories/{id}/broadcast` — broadcast consentido de campanha
//! (ÁGORA F3, #60, migration 0655). O carro-chefe.
//!
//! Um diretório **municipal** manda uma mensagem à sua base consentida. A plataforma resolve
//! QUEM autorizou (cruzando o consentimento 4-níveis `citizen_campaign_consent` 0654 com o
//! **domicílio** do cidadão `citizen.uf/municipio_ibge` 0652) e envia **via INTERCOMS**
//! (`SmtpProvider`) — só a essas pessoas, nunca expondo a lista. Opt-out no rodapé. Cooldown de
//! 24h por diretório (anti-spam). Registrado em `campaign_broadcast` (auditoria + rate-limit).
//!
//! Gating: admin de plataforma (`party.manage`/`administrator`) OU `party_administrator` do
//! partido (nacional ou deste diretório). English API por ADR-0013. Runtime queries.

use axum::extract::{Json, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use dsoc_admin::permissions::keys;
use dsoc_admin::AdminService;
use dsoc_api_contract::ApiResponse;
use dsoc_app::{AppState, CallerId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::intercoms::{MessageSender, OutboundMessage, SmtpProvider};

/// Teto de destinatários por broadcast (defesa; a fase municipal é de baixo volume).
const MAX_RECIPIENTS: i64 = 5000;
/// Cooldown por diretório.
const COOLDOWN_HOURS: i64 = 24;
const MAX_SUBJECT: usize = 160;
const MAX_BODY: usize = 4000;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route(
            "/admin/parties/{sigla}/directories/{id}/broadcast",
            post(broadcast),
        )
        .with_state(state)
}

fn fail(status: StatusCode, code: &str, msg: &str) -> Response {
    (status, Json(ApiResponse::<()>::fail(code, msg))).into_response()
}
fn storage_error() -> Response {
    fail(StatusCode::INTERNAL_SERVER_ERROR, "storage_error", "Erro interno.")
}

#[derive(Deserialize)]
struct BroadcastBody {
    subject: String,
    body: String,
}

#[derive(Serialize)]
struct BroadcastResult {
    recipients: i64,
    broadcast_id: Uuid,
}

/// Autorizado se admin de plataforma OU party_administrator (nacional ou deste diretório).
async fn authorized(
    state: &AppState,
    caller: &CallerId,
    sigla: &str,
    directory_id: Uuid,
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
    let is_party_admin: bool = sqlx::query_scalar(
        r"SELECT EXISTS(
             SELECT 1 FROM party_administrator
              WHERE org_id = $1 AND party_sigla = $2 AND citizen_id = $3
                AND (directory_id IS NULL OR directory_id = $4))",
    )
    .bind(org)
    .bind(sigla)
    .bind(citizen)
    .bind(directory_id)
    .fetch_one(&state.db)
    .await
    .map_err(|_| storage_error())?;
    Ok(is_party_admin)
}

async fn broadcast(
    State(state): State<AppState>,
    caller: CallerId,
    Path((sigla, directory_id)): Path<(String, Uuid)>,
    Json(body): Json<BroadcastBody>,
) -> Response {
    let org = caller.org.as_uuid();
    let sent_by = caller.citizen.as_uuid();

    match authorized(&state, &caller, &sigla, directory_id).await {
        Ok(true) => {}
        Ok(false) => {
            return fail(StatusCode::FORBIDDEN, "http_403", "Você não administra este partido/diretório.")
        }
        Err(r) => return r,
    }

    let subject = body.subject.trim().to_owned();
    let msg_body = body.body.trim().to_owned();
    if subject.is_empty() || subject.chars().count() > MAX_SUBJECT {
        return fail(StatusCode::BAD_REQUEST, "invalid_subject", "Assunto de 1 a 160 caracteres.");
    }
    if msg_body.is_empty() || msg_body.chars().count() > MAX_BODY {
        return fail(StatusCode::BAD_REQUEST, "invalid_body", "Mensagem de 1 a 4000 caracteres.");
    }

    // O diretório precisa existir na org, ser deste partido e ser MUNICIPAL (uf + municipio).
    let dir: Option<(String, Option<String>, Option<String>)> = match sqlx::query_as(
        r"SELECT esfera, uf, municipio FROM party_directory
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
            tracing::error!(?err, "broadcast: load directory");
            return storage_error();
        }
    };
    let Some((esfera, uf, municipio)) = dir else {
        return fail(StatusCode::NOT_FOUND, "directory_not_found", "Diretório não encontrado.");
    };
    let (Some(uf), Some(municipio)) = (uf, municipio) else {
        return fail(
            StatusCode::BAD_REQUEST,
            "not_municipal",
            "Por ora o broadcast é só para diretórios municipais.",
        );
    };
    let _ = esfera;

    // Cooldown por diretório.
    let recent: bool = match sqlx::query_scalar(
        r"SELECT EXISTS(SELECT 1 FROM campaign_broadcast
                         WHERE directory_id = $1 AND created_at > now() - ($2 || ' hours')::interval)",
    )
    .bind(directory_id)
    .bind(COOLDOWN_HOURS.to_string())
    .fetch_one(&state.db)
    .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "broadcast: cooldown check");
            return storage_error();
        }
    };
    if recent {
        return fail(
            StatusCode::TOO_MANY_REQUESTS,
            "cooldown",
            "Este diretório já enviou um broadcast nas últimas 24h.",
        );
    }

    // Resolução de alcance: cidadãos que RESIDEM no município do diretório E consentiram num
    // nível que cobre este diretório. Comparação de município case-insensitive (o texto do
    // diretório pode divergir do IBGE em caixa).
    let emails: Vec<(String,)> = match sqlx::query_as(
        r"SELECT DISTINCT ac.email
            FROM citizen c
            JOIN municipio_ibge mi ON mi.codigo_ibge = c.municipio_ibge
            JOIN auth_credential ac ON ac.citizen_id = c.id
           WHERE c.org_id = $1 AND c.uf = $2 AND upper(mi.nome) = upper($3)
             AND ac.email IS NOT NULL AND ac.email <> ''
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
            tracing::error!(?err, "broadcast: resolve reach");
            return storage_error();
        }
    };
    let recipients = emails.len() as i64;

    // Registra o broadcast (auditoria + rate-limit) ANTES de enviar.
    let broadcast_id: Uuid = match sqlx::query_scalar(
        r"INSERT INTO campaign_broadcast (org_id, party_sigla, directory_id, sent_by, subject, body, recipients)
          VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
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
            tracing::error!(?err, "broadcast: insert record");
            return storage_error();
        }
    };

    // Envio em background (best-effort) via INTERCOMS — não trava a requisição.
    if let Some(sender) = SmtpProvider::from_env() {
        let footer = format!(
            "\n\n—\nVocê recebe isto porque autorizou comunicações de campanha do {sigla}. \
             Para ajustar ou revogar: https://democracia.social.br/configuracoes#campanha"
        );
        let full = format!("{msg_body}{footer}");
        let subj = subject.clone();
        let list: Vec<String> = emails.into_iter().map(|(e,)| e).collect();
        tokio::spawn(async move {
            for to in list {
                if let Err(err) = sender
                    .send(&OutboundMessage::email(&to, &subj, &full))
                    .await
                {
                    tracing::warn!(?err, "broadcast: envio falhou (best-effort)");
                }
            }
        });
    } else {
        tracing::warn!("broadcast: SMTP não configurado — {recipients} destinatários não notificados");
    }

    (
        StatusCode::OK,
        Json(ApiResponse::ok(BroadcastResult { recipients, broadcast_id })),
    )
        .into_response()
}

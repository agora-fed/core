//! Endpoints LGPD — direitos do titular (art. 18 Lei 13.709/2018).
//!
//! Duas ações que o cidadão pode acionar da UI de Configurações:
//!
//! - `GET  /me/lgpd/export` — devolve um JSON completo com todos os dados
//!   pessoais dele armazenados (art. 18 II e V).
//! - `POST /me/lgpd/delete-account` — soft-delete: marca `deleted_at`,
//!   limpa PII (email, CPF, senha, título, gov.br, avatar), invalida
//!   todas as sessões. Conteúdo público (propostas, comentários, votos)
//!   fica com autor anonimizado (interesse público em accountability
//!   histórica de mandato — LGPD art. 16).

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/me/lgpd/export", get(export))
        .route("/me/lgpd/delete-account", post(delete_account))
        .with_state(state)
}

fn caller(headers: &HeaderMap) -> Option<Uuid> {
    headers
        .get("x-dsoc-citizen-id")?
        .to_str()
        .ok()?
        .parse()
        .ok()
}

fn unauth() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(ApiResponse::<()>::fail(
            "unauthorized",
            "Autenticação necessária.",
        )),
    )
        .into_response()
}

async fn export(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(cid) = caller(&headers) else {
        return unauth();
    };
    let db = &state.db;
    let result = build_export(db, cid).await;
    match result {
        Ok(v) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json".to_owned())],
            Json(ApiResponse::ok(v)),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "lgpd export failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::fail("storage_error", "Erro interno.")),
            )
                .into_response()
        }
    }
}

async fn build_export(db: &PgPool, citizen_id: Uuid) -> Result<serde_json::Value, sqlx::Error> {
    // Citizen row completo.
    let citizen: serde_json::Value =
        sqlx::query_scalar(r"SELECT to_jsonb(c) - 'oidc_subject' FROM citizen c WHERE c.id = $1")
            .bind(citizen_id)
            .fetch_one(db)
            .await?;

    // Credentials (só e-mail; jamais o hash da senha).
    let credentials: Vec<serde_json::Value> = sqlx::query_scalar(
        r"SELECT jsonb_build_object(
              'email', email,
              'cpf_status', cpf_status,
              'created_at', created_at
          )
          FROM auth_credential WHERE citizen_id = $1",
    )
    .bind(citizen_id)
    .fetch_all(db)
    .await?;

    // Sessions ativas.
    let sessions: Vec<serde_json::Value> = sqlx::query_scalar(
        r"SELECT jsonb_build_object(
              'id', id, 'issued_at', issued_at, 'expires_at', expires_at
          )
          FROM auth_session WHERE citizen_id = $1",
    )
    .bind(citizen_id)
    .fetch_all(db)
    .await?;

    // Propostas do cidadão.
    let proposals: Vec<serde_json::Value> = sqlx::query_scalar(
        r"SELECT jsonb_build_object(
              'id', id, 'title', title, 'body', body, 'status', status,
              'urgencia', urgencia, 'support_count', support_count,
              'threshold', threshold, 'threshold_crossed_at', threshold_crossed_at,
              'published_at', published_at, 'created_at', created_at,
              'mandate_id', mandate_id
          )
          FROM proposal WHERE author_citizen_id = $1
          ORDER BY created_at DESC",
    )
    .bind(citizen_id)
    .fetch_all(db)
    .await?;

    // Votos que eu dei.
    let votes: Vec<serde_json::Value> = sqlx::query_scalar(
        r"SELECT jsonb_build_object(
              'id', id, 'proposal_id', proposal_id, 'created_at', created_at
          )
          FROM votes_vote WHERE citizen_id = $1",
    )
    .bind(citizen_id)
    .fetch_all(db)
    .await?;

    // Emendas propostas por mim.
    let amendments: Vec<serde_json::Value> = sqlx::query_scalar(
        r"SELECT jsonb_build_object(
              'id', id, 'proposal_id', proposal_id, 'body', body,
              'rationale', rationale, 'status', status, 'created_at', created_at
          )
          FROM proposal_amendment WHERE author_id = $1",
    )
    .bind(citizen_id)
    .fetch_all(db)
    .await?;

    // Notificações que recebi.
    let notifications: Vec<serde_json::Value> = sqlx::query_scalar(
        r"SELECT jsonb_build_object(
              'id', id, 'kind', kind, 'object_uri', object_uri,
              'object_preview', object_preview, 'created_at', created_at,
              'read_at', read_at
          )
          FROM user_notification WHERE citizen_id = $1
          ORDER BY created_at DESC",
    )
    .bind(citizen_id)
    .fetch_all(db)
    .await?;

    // Web push subs.
    let push_subs: Vec<serde_json::Value> = sqlx::query_scalar(
        r"SELECT jsonb_build_object(
              'id', id, 'endpoint', endpoint,
              'user_agent', user_agent, 'created_at', created_at,
              'dead_at', dead_at
          )
          FROM notify_web_push_subscription WHERE citizen_id = $1",
    )
    .bind(citizen_id)
    .fetch_all(db)
    .await?;

    Ok(json!({
        "$schema": "https://democracia.social.br/schemas/lgpd-export-v1.json",
        "generated_at": chrono::Utc::now(),
        "citizen": citizen,
        "credentials": credentials,
        "sessions": sessions,
        "proposals": proposals,
        "votes": votes,
        "amendments": amendments,
        "notifications": notifications,
        "push_subscriptions": push_subs,
        "note": "Este é o pacote completo dos seus dados pessoais na DemocraciaBR. Interesse público em conteúdo já publicado (proposta, voto) pode fazer com que o artefato permaneça no registro histórico após exclusão da conta — apenas seu vínculo com ele é removido."
    }))
}

async fn delete_account(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(cid) = caller(&headers) else {
        return unauth();
    };
    let db = &state.db;

    // Uma transação com todas as limpezas.
    let mut tx = match db.begin().await {
        Ok(t) => t,
        Err(err) => return storage(err),
    };
    // 1. Marca deleted_at + apaga PII editável na citizen.
    if let Err(err) = sqlx::query(
        r"UPDATE citizen
             SET deleted_at = now(),
                 display_name = NULL,
                 bio = NULL,
                 handle = NULL,
                 avatar_object_key = NULL,
                 cover_object_key = NULL,
                 titulo_eleitor = NULL,
                 titulo_status = NULL,
                 govbr_sub = NULL,
                 legal_name = NULL,
                 is_public = false,
                 profile_updated_at = now()
           WHERE id = $1",
    )
    .bind(cid)
    .execute(&mut *tx)
    .await
    {
        return storage(err);
    }
    // 2. Limpa credenciais (email, cpf, senha_hash).
    if let Err(err) = sqlx::query("DELETE FROM auth_credential WHERE citizen_id = $1")
        .bind(cid)
        .execute(&mut *tx)
        .await
    {
        return storage(err);
    }
    // 3. Mata todas as sessões.
    if let Err(err) = sqlx::query("DELETE FROM auth_session WHERE citizen_id = $1")
        .bind(cid)
        .execute(&mut *tx)
        .await
    {
        return storage(err);
    }
    // 4. Push subs.
    if let Err(err) = sqlx::query("DELETE FROM notify_web_push_subscription WHERE citizen_id = $1")
        .bind(cid)
        .execute(&mut *tx)
        .await
    {
        return storage(err);
    }
    // 5. Pending signup (se tiver).
    if let Err(err) = sqlx::query(
        "DELETE FROM auth_pending_signup WHERE email IN (
            SELECT email FROM auth_credential WHERE citizen_id = $1
        )",
    )
    .bind(cid)
    .execute(&mut *tx)
    .await
    {
        // Não crítico — segue mesmo se falhar.
        tracing::warn!(?err, "lgpd delete: pending cleanup falhou");
    }

    if let Err(err) = tx.commit().await {
        return storage(err);
    }

    // Cookie kill.
    let kill = "dsoc_session=; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=0";
    (
        StatusCode::OK,
        [(header::SET_COOKIE, kill)],
        Json(ApiResponse::<()>::ok(())),
    )
        .into_response()
}

fn storage(err: sqlx::Error) -> Response {
    tracing::error!(?err, "lgpd delete failed");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiResponse::<()>::fail("storage_error", "Erro interno.")),
    )
        .into_response()
}

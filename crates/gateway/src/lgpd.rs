//! Endpoints LGPD — direitos do titular (art. 18 Lei 13.709/2018).
//!
//! Two actions the citizen can trigger from the Settings UI:
//!
//! - `GET  /me/lgpd/export` — returns a complete JSON with every piece of their
//!   personal data we hold (art. 18 II and V).
//! - `POST /me/lgpd/delete-account` — soft-delete: marks `deleted_at`,
//!   wipes PII (e-mail, identity document, password, electoral registry, gov.br, avatar), invalidates
//!   every session. Public content (proposals, comments, votes)
//!   stays with an anonymized author (the public interest in historical
//!   mandate accountability — LGPD art. 16).

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
    // An ALLOWLIST built from the PII registry (issue #16). This used to be
    // `to_jsonb(c) - 'oidc_subject'` — a one-field blocklist, so every column added
    // after it was written was exported by default, the TOTP secret included.
    let sql = format!(
        "SELECT {} FROM citizen c WHERE c.id = $1",
        crate::pii_registry::export_json_object()
    );
    let citizen: serde_json::Value = sqlx::query_scalar(&sql)
        .bind(citizen_id)
        .fetch_one(db)
        .await?;

    // Credentials (e-mail only; never the password hash).
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

    // The citizen's proposals.
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

    // Votes I cast.
    let votes: Vec<serde_json::Value> = sqlx::query_scalar(
        r"SELECT jsonb_build_object(
              'id', id, 'proposal_id', proposal_id, 'created_at', created_at
          )
          FROM votes_vote WHERE citizen_id = $1",
    )
    .bind(citizen_id)
    .fetch_all(db)
    .await?;

    // Amendments I proposed.
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

    // Notifications I received.
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

    // A single transaction with every cleanup.
    let mut tx = match db.begin().await {
        Ok(t) => t,
        Err(err) => return storage(err),
    };
    // 1. Mark deleted_at + wipe editable PII on citizen.
    // Generated from the PII registry (issue #16). The hand-written list this
    // replaces had fallen behind the schema: phone, TOTP, birth date and domicile all
    // survived a deletion request.
    let erase_sql = format!(
        "UPDATE citizen SET deleted_at = now(), profile_updated_at = now(), {} WHERE id = $1",
        crate::pii_registry::erase_set_clause()
    );
    if let Err(err) = sqlx::query(&erase_sql).bind(cid).execute(&mut *tx).await {
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
    // 3. Kill every session.
    if let Err(err) = sqlx::query("DELETE FROM auth_session WHERE citizen_id = $1")
        .bind(cid)
        .execute(&mut *tx)
        .await
    {
        return storage(err);
    }
    // 4. Push subs.
    // Credential and identifier material that survived erasure before (issue #16).
    // These are NOT accountability records — they are keys and copies, and a person
    // who asked to be forgotten should not leave behind a working second factor.
    //
    // Deliberately NOT here: votes, comments, proposals and the moderation record.
    // Those are other people's context and the platform's own acts; LGPD art. 16
    // covers retaining them, and erasing them would rewrite public deliberation
    // rather than protect a person.
    for table in [
        // Working 2FA recovery codes.
        "totp_recovery_code",
        // The ActivityPub signing key — whoever holds it can speak as this actor.
        "citizen_actor_key",
        // A copy of the phone number plus the OTP hash.
        "phone_otp",
        // Reset tokens carry a request IP alongside the token.
        "auth_password_reset",
        // Live API credentials.
        "oauth_access_token",
        "oauth_authorization_code",
        // Push identifiers tie the account to a physical device.
        "notify_device_token",
    ] {
        // The table name comes from this literal list, never from input.
        let sql = format!("DELETE FROM {table} WHERE citizen_id = $1");
        if let Err(err) = sqlx::query(&sql).bind(cid).execute(&mut *tx).await {
            return storage(err);
        }
    }
    if let Err(err) = sqlx::query("DELETE FROM notify_web_push_subscription WHERE citizen_id = $1")
        .bind(cid)
        .execute(&mut *tx)
        .await
    {
        return storage(err);
    }
    // 5. Pending signup (if any).
    if let Err(err) = sqlx::query(
        "DELETE FROM auth_pending_signup WHERE email IN (
            SELECT email FROM auth_credential WHERE citizen_id = $1
        )",
    )
    .bind(cid)
    .execute(&mut *tx)
    .await
    {
        // Not critical — proceed even if it fails.
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

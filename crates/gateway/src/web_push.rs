//! # Web Push (RFC 8291) — envio de notificações pro navegador do cidadão.
//!
//! 0.25.0-fediverso: complementa o feed `user_notification` com push real.
//! Quando `civic_notify.rs` insere uma linha, a gente também dispara um push
//! pra qualquer subscription ativa do citizen — o navegador mostra
//! notificação nativa (mesmo com a aba fechada, via service worker).
//!
//! Config via env (nunca hardcoded, PLAN.md princípio 8):
//! - `VAPID_PUBLIC_KEY` — chave pública EC P-256, base64url (65 bytes uncompressed).
//! - `VAPID_PRIVATE_KEY` — chave privada, base64url (32 bytes).
//! - `VAPID_SUBJECT` — `mailto:sistema@democracia.social.br` ou similar.
//!
//! Sem VAPID configurado, a gente **não** dispara push (log em INFO). A
//! subscription do lado do front continua funcionando — só não chega
//! notificação nativa.

use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use dsoc_api_contract::ApiResponse;
use dsoc_app::{AppState, CallerId};
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// HTTP surface
// ---------------------------------------------------------------------------

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/me/push-subscriptions", post(subscribe))
        .route(
            "/me/push-subscriptions/vapid-public-key",
            axum::routing::get(vapid_pub),
        )
        .with_state(state)
}

/// Body do `POST /me/push-subscriptions`. Casada com a shape produzida pelo
/// `PushSubscription.toJSON()` do navegador (endpoint + keys.{p256dh, auth}).
#[derive(Debug, Deserialize)]
struct SubscribeRequest {
    endpoint: String,
    keys: SubscribeKeys,
    #[serde(default)]
    user_agent: Option<String>,
}
#[derive(Debug, Deserialize)]
struct SubscribeKeys {
    p256dh: String,
    auth: String,
}

async fn subscribe(
    State(state): State<AppState>,
    caller: CallerId,
    Json(body): Json<SubscribeRequest>,
) -> Response {
    let now = state.clock.now();
    let res = sqlx::query(
        r"INSERT INTO notify_web_push_subscription
             (id, citizen_id, endpoint, p256dh, auth, user_agent, created_at, dead_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, NULL)
           ON CONFLICT (citizen_id, endpoint) DO UPDATE
              SET p256dh = EXCLUDED.p256dh,
                  auth   = EXCLUDED.auth,
                  user_agent = EXCLUDED.user_agent,
                  dead_at = NULL",
    )
    .bind(Uuid::now_v7())
    .bind(caller.citizen.as_uuid())
    .bind(&body.endpoint)
    .bind(&body.keys.p256dh)
    .bind(&body.keys.auth)
    .bind(&body.user_agent)
    .bind(now)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => (StatusCode::CREATED, Json(ApiResponse::<()>::ok(()))).into_response(),
        Err(err) => {
            tracing::error!(error = ?err, "web_push subscribe failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::fail("storage_error", "Erro interno.")),
            )
                .into_response()
        }
    }
}

/// GET público: navegador precisa da chave pública em base64url pra criar a
/// subscription. Retorna 503 se não configurada — o front esconde o botão.
async fn vapid_pub() -> Response {
    match std::env::var("VAPID_PUBLIC_KEY")
        .ok()
        .filter(|s| !s.is_empty())
    {
        Some(key) => (
            StatusCode::OK,
            Json(ApiResponse::ok(json!({ "public_key": key }))),
        )
            .into_response(),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiResponse::<()>::fail(
                "vapid_unconfigured",
                "Notificações push ainda não configuradas nesta instância.",
            )),
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------
// Envio de push (chamado por civic_notify após insert)
// ---------------------------------------------------------------------------

/// Dispatch de todas as subscriptions ativas do cidadão. Idempotente-ish: um
/// endpoint que retornar 410 Gone é marcado `dead_at = now` e nunca mais
/// tenta. Falha isolada não bloqueia outras subs.
///
/// `payload_json` chega ao service worker como `event.data.text()` — front
/// espera `{title, body, url}` e mostra `showNotification(title, {body, ...})`.
pub async fn send_to_citizen(db: &PgPool, citizen_id: Uuid, payload_json: &str) {
    let cfg = match VapidConfig::from_env() {
        Some(c) => c,
        None => {
            tracing::debug!("web_push: VAPID unset, pulando envio");
            return;
        }
    };
    let subs: Vec<PushSubRow> = match sqlx::query_as(
        r"SELECT id, endpoint, p256dh, auth
            FROM notify_web_push_subscription
           WHERE citizen_id = $1 AND dead_at IS NULL",
    )
    .bind(citizen_id)
    .fetch_all(db)
    .await
    {
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!(error = ?err, "web_push: query subs falhou");
            return;
        }
    };
    for sub in subs {
        let db = db.clone();
        let cfg = cfg.clone();
        let payload = payload_json.to_owned();
        tokio::spawn(async move {
            if let Err(WebPushSendError::Expired) = send_one(&cfg, &sub, &payload).await {
                let _ = sqlx::query(
                    "UPDATE notify_web_push_subscription SET dead_at = now() WHERE id = $1",
                )
                .bind(sub.id)
                .execute(&db)
                .await;
            }
        });
    }
}

#[derive(sqlx::FromRow, Debug, Clone)]
struct PushSubRow {
    id: Uuid,
    endpoint: String,
    p256dh: String,
    auth: String,
}

#[derive(Clone)]
struct VapidConfig {
    private_key: String,
    subject: String,
}

impl VapidConfig {
    fn from_env() -> Option<Self> {
        // Public key não é consumida aqui (o front pede via GET /vapid-public-key
        // e usa direto pra criar a subscription); só validamos que existe pra
        // não enviar push com config parcial.
        let _pub = std::env::var("VAPID_PUBLIC_KEY")
            .ok()
            .filter(|s| !s.is_empty())?;
        let private_key = std::env::var("VAPID_PRIVATE_KEY")
            .ok()
            .filter(|s| !s.is_empty())?;
        let subject = std::env::var("VAPID_SUBJECT")
            .unwrap_or_else(|_| "mailto:sistema@democracia.social.br".to_owned());
        Some(Self {
            private_key,
            subject,
        })
    }
}

#[derive(Debug)]
enum WebPushSendError {
    Expired,
    Other,
}

async fn send_one(
    cfg: &VapidConfig,
    sub: &PushSubRow,
    payload: &str,
) -> std::result::Result<(), WebPushSendError> {
    use web_push::{
        ContentEncoding, HyperWebPushClient, SubscriptionInfo, SubscriptionKeys,
        VapidSignatureBuilder, WebPushClient, WebPushMessageBuilder, URL_SAFE_NO_PAD,
    };

    let sub_info = SubscriptionInfo {
        endpoint: sub.endpoint.clone(),
        keys: SubscriptionKeys {
            p256dh: sub.p256dh.clone(),
            auth: sub.auth.clone(),
        },
    };
    // Priv key: base64url no-pad dos 32 bytes puros.
    let mut sig_builder =
        VapidSignatureBuilder::from_base64(&cfg.private_key, URL_SAFE_NO_PAD, &sub_info)
            .map_err(|_| WebPushSendError::Other)?;
    sig_builder.add_claim("sub", cfg.subject.as_str());
    let signature = sig_builder.build().map_err(|_| WebPushSendError::Other)?;

    let mut builder = WebPushMessageBuilder::new(&sub_info);
    builder.set_payload(ContentEncoding::Aes128Gcm, payload.as_bytes());
    builder.set_vapid_signature(signature);
    let msg = builder.build().map_err(|_| WebPushSendError::Other)?;

    let client = HyperWebPushClient::new();
    match client.send(msg).await {
        Ok(_) => Ok(()),
        Err(web_push::WebPushError::EndpointNotValid)
        | Err(web_push::WebPushError::EndpointNotFound) => Err(WebPushSendError::Expired),
        Err(err) => {
            tracing::warn!(?err, endpoint = %sub.endpoint, "web_push: send failed");
            Err(WebPushSendError::Other)
        }
    }
}

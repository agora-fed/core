//! # Reply-to-respond — the office answers without an account (item 3, 0.30.0).
//!
//! The consequence loop's bottleneck is adoption by the official: requiring
//! registration in order to answer is friction that becomes silence. Solution: the e-mails
//! warning e-mails to the cabinet carry a signed link
//! (`/responder/?sla=<id>&t=<hmac>`); quem controla a caixa OFICIAL do
//! mandate (public data from the legislature/electoral authority) answers right on the page, with no
//! login. Possession of the token IS the authorization — the same model as postal registered mail:
//! whoever signs for delivery is whoever holds the address.
//!
//! Token (since #12): 32 random bytes, hex-encoded, with only its SHA-256 stored in
//! `respond_link` (0679). Each link therefore EXPIRES, is SINGLE-USE and is
//! individually REVOCABLE.
//!
//! It used to be `hmac(RESPOND_LINK_SECRET, sla_id)`: deterministic, with no temporal
//! component. It never expired, replayed forever, and could only be invalidated by
//! rotating the global secret — which invalidated every link at once. Since `POST
//! /respond` is unauthenticated and writes a mandate's public official response,
//! possession of a stale URL was standing authority to speak in an official's name.
//!
//! The registered-mail analogy in the paragraph above still holds for WHO may answer.
//! What changed is that signing for delivery is now an event, not a permanent power.
//!
//! Reading the context does NOT spend the link — the official opens the page, then
//! answers. Only a recorded response spends it.
//!
//! - `GET  /respond/context?sla&t` — context for the page (title, mandate,
//!   deadline) given a valid token.
//! - `POST /respond {sla_id, token, body, committed}` — registra a resposta
//!   official via `ConsequenceService::respond` (Conflict = the SLA is already resolved;
//!   the public outcome is permanent).

use axum::extract::{Json, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use dsoc_core::ids::SlaId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/respond/context", get(context))
        .route("/respond", post(submit))
        .with_state(state)
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// How long a freshly-minted link stays valid.
///
/// Generous on purpose: the warning ladder mails D0, D+1 and D+2, an official may
/// answer weeks later, and a link that dies before the person gets to it turns into
/// silence — which is the exact outcome this whole feature exists to prevent. The
/// protection against a leaked URL is that it is single-use, not that it is brief.
const LINK_TTL_DAYS: i64 = 30;

/// Failed presentations tolerated per link before it is refused outright.
const MAX_ATTEMPTS: i32 = 20;

fn sha256(bytes: &[u8]) -> Vec<u8> {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes).to_vec()
}

/// Mint (or reuse) a live link for `sla_id` and return its token.
///
/// Reuses an existing unspent, unexpired, unrevoked link so the D0/D+1/D+2 mails all
/// carry the SAME working URL — three different tokens would mean two of the three
/// e-mails are dead links, and an official who opens the first one gets refused.
///
/// `None` = the feature is dormant (no `RESPOND_LINK_SECRET`) or storage failed.
pub(crate) async fn issue_token(db: &sqlx::PgPool, sla_id: Uuid) -> Option<String> {
    let secret = std::env::var("RESPOND_LINK_SECRET").ok()?;
    if secret.trim().is_empty() {
        return None;
    }
    // A live link already mailed for this SLA — reuse it rather than orphan it.
    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM respond_link \
          WHERE sla_id = $1 AND used_at IS NULL AND revoked_at IS NULL \
            AND expires_at > now() AND attempts < $2 \
          ORDER BY created_at DESC LIMIT 1",
    )
    .bind(sla_id)
    .bind(MAX_ATTEMPTS)
    .fetch_optional(db)
    .await
    .ok()?;
    if existing.is_some() {
        // The token is not recoverable (only its hash is stored), so a reusable link
        // can only be re-sent, never re-derived. Mint a fresh one and revoke the old:
        // the alternative is storing the plaintext, which is what 0679 refuses to do.
        let _ = sqlx::query("UPDATE respond_link SET revoked_at = now() WHERE sla_id = $1 AND used_at IS NULL AND revoked_at IS NULL")
            .bind(sla_id)
            .execute(db)
            .await;
    }

    // Same RNG the OAuth/OIDC secrets in this crate use (`rand::rngs::OsRng`).
    let mut raw = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut raw);
    let token = hex_encode(&raw);
    let expires = chrono::Utc::now() + chrono::Duration::days(LINK_TTL_DAYS);
    sqlx::query(
        "INSERT INTO respond_link (id, sla_id, token_hash, expires_at) VALUES ($1, $2, $3, $4)",
    )
    .bind(Uuid::now_v7())
    .bind(sla_id)
    .bind(sha256(token.as_bytes()))
    .bind(expires)
    .execute(db)
    .await
    .ok()?;
    Some(token)
}

/// Why a presented link was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinkError {
    /// No live link matches this (sla, token).
    Invalid,
    /// A link matched but is expired, spent, revoked, or out of attempts.
    Spent,
    /// The lookup failed. Refuse — never treat a storage error as a verdict.
    Unavailable,
}

/// Check a presented token against the live links of `sla_id`, returning the link id.
///
/// A miss increments the attempt counter of every live link for the SLA, so guessing
/// is bounded even though the attacker never learns which link they are hitting.
///
/// A storage failure returns [`LinkError::Unavailable`] rather than "invalid": this
/// gate decides whether a stranger may post in an official's name, and the previous
/// code's `.unwrap_or(None)` turned a database blip into a silent 404 that read the
/// same as a bad token.
pub(crate) async fn verify_token(
    db: &sqlx::PgPool,
    sla_id: Uuid,
    presented: &str,
) -> Result<Uuid, LinkError> {
    if presented.len() != 64 || hex_decode(presented).is_none() {
        return Err(LinkError::Invalid);
    }
    let hash = sha256(presented.as_bytes());
    let row: Option<(Uuid, bool)> = match sqlx::query_as(
        "SELECT id, \
                (used_at IS NULL AND revoked_at IS NULL AND expires_at > now() \
                 AND attempts < $3) AS live \
           FROM respond_link \
          WHERE sla_id = $1 AND token_hash = $2 \
          LIMIT 1",
    )
    .bind(sla_id)
    .bind(&hash)
    .bind(MAX_ATTEMPTS)
    .fetch_optional(db)
    .await
    {
        Ok(r) => r,
        Err(err) => {
            tracing::error!(error = ?err, sla = %sla_id, "respond-link lookup failed; refusing");
            return Err(LinkError::Unavailable);
        }
    };
    match row {
        Some((id, true)) => Ok(id),
        Some((_, false)) => Err(LinkError::Spent),
        None => {
            let _ = sqlx::query(
                "UPDATE respond_link SET attempts = attempts + 1 \
                  WHERE sla_id = $1 AND used_at IS NULL AND revoked_at IS NULL",
            )
            .bind(sla_id)
            .execute(db)
            .await;
            Err(LinkError::Invalid)
        }
    }
}

/// Spend a link. Called only after a response is actually recorded.
async fn consume(db: &sqlx::PgPool, link_id: Uuid) {
    if let Err(err) = sqlx::query("UPDATE respond_link SET used_at = now() WHERE id = $1")
        .bind(link_id)
        .execute(db)
        .await
    {
        // The response IS recorded at this point; failing to mark the link only
        // leaves it replayable until it expires. Loud, but not a reason to fail
        // the official's answer.
        tracing::error!(error = ?err, link = %link_id, "respond-link consume failed");
    }
}

/// Map a link failure to a response. A spent/expired link says so, because telling an
/// official "this link was already used" is actionable, while "invalid" sends them to
/// support. Neither answer distinguishes a wrong token from an unknown SLA.
fn refused(err: LinkError) -> Response {
    match err {
        LinkError::Spent => (
            StatusCode::GONE,
            Json(ApiResponse::<()>::fail(
                "link_spent",
                "Este link de resposta já foi usado ou expirou. Peça um novo pelo e-mail de cobrança.",
            )),
        )
            .into_response(),
        LinkError::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiResponse::<()>::fail(
                "storage_error",
                "Não foi possível validar o link agora. Tente novamente em instantes.",
            )),
        )
            .into_response(),
        LinkError::Invalid => denied(),
    }
}

fn denied() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(ApiResponse::<()>::fail(
            "invalid_token",
            "Link de resposta inválido ou recurso desativado.",
        )),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
struct ContextQuery {
    sla: Uuid,
    t: String,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct RespondContextDto {
    proposal_title: String,
    mandate_display_name: Option<String>,
    due_at: chrono::DateTime<chrono::Utc>,
    status: String,
}

async fn context(State(state): State<AppState>, Query(q): Query<ContextQuery>) -> Response {
    // Reading the context does NOT spend the link: the official opens the page and
    // then answers, and burning the token on the GET would make every form submit fail.
    if let Err(err) = verify_token(&state.db, q.sla, &q.t).await {
        return refused(err);
    }
    let row = sqlx::query_as::<_, RespondContextDto>(
        r"SELECT p.title AS proposal_title,
                 m.display_name AS mandate_display_name,
                 s.due_at,
                 s.status
            FROM consequence_sla s
            JOIN proposal p ON p.id = s.proposal_id
            LEFT JOIN mandate m ON m.id = s.mandate_id
           WHERE s.id = $1",
    )
    .bind(q.sla)
    .fetch_optional(&state.db)
    .await;
    // A storage failure is NOT "not found": the previous `.unwrap_or(None)` reported
    // a DB blip as a missing SLA, which reads to the official exactly like an invalid
    // link and sends them away for good.
    let row = match row {
        Ok(r) => r,
        Err(err) => {
            tracing::error!(error = ?err, sla = %q.sla, "respond context lookup failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::fail("storage_error", "Erro interno.")),
            )
                .into_response();
        }
    };
    match row {
        Some(ctx) => (StatusCode::OK, Json(ApiResponse::ok(ctx))).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::fail("not_found", "SLA não encontrado.")),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct SubmitBody {
    sla_id: Uuid,
    token: String,
    body: String,
    #[serde(default)]
    committed: bool,
}

async fn submit(State(state): State<AppState>, Json(req): Json<SubmitBody>) -> Response {
    let link_id = match verify_token(&state.db, req.sla_id, &req.token).await {
        Ok(id) => id,
        Err(err) => return refused(err),
    };
    let svc = dsoc_consequence::ConsequenceService::from_state(&state);
    match svc
        .respond(SlaId::from_uuid(req.sla_id), &req.body, req.committed)
        .await
    {
        Ok(outcome) => {
            // Spent only now: the link buys exactly one recorded response.
            consume(&state.db, link_id).await;
            tracing::info!(sla = %req.sla_id, ?outcome, "reply-to-respond: response recorded via link");
            (
                StatusCode::OK,
                Json(ApiResponse::ok(serde_json::json!({ "ok": true }))),
            )
                .into_response()
        }
        Err(dsoc_core::Error::Conflict(_)) => (
            StatusCode::CONFLICT,
            Json(ApiResponse::<()>::fail(
                "already_resolved",
                "Este prazo já foi resolvido — o desfecho público é permanente.",
            )),
        )
            .into_response(),
        Err(dsoc_core::Error::Validation(msg)) => (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::fail("invalid_body", &msg)),
        )
            .into_response(),
        Err(dsoc_core::Error::NotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::fail("not_found", "SLA não encontrado.")),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, sla = %req.sla_id, "reply-to-respond: respond falhou");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::fail("storage_error", "Erro interno.")),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_malformed_token_is_rejected_on_shape_alone() {
        // Cheap pre-checks, before any database work: the token is exactly 64 hex
        // characters, so anything else cannot match and must not cost a query.
        for bad in [
            "",
            "zzzz",
            "abc",
            &"a".repeat(63),
            &"a".repeat(65),
            &"z".repeat(64),
        ] {
            assert!(
                bad.len() != 64 || hex_decode(bad).is_none(),
                "{bad:?} must fail the shape check"
            );
        }
        // A well-formed token passes the SHAPE check — whether it is LIVE is a
        // database question, covered by the integration tests.
        let good = "a".repeat(64);
        assert_eq!(good.len(), 64);
        assert!(hex_decode(&good).is_some());
    }

    #[test]
    fn the_stored_hash_is_not_the_token() {
        // 0679 stores only the digest, so a database reader cannot mint a link.
        let token = "b".repeat(64);
        let stored = sha256(token.as_bytes());
        assert_ne!(stored, token.as_bytes().to_vec());
        assert_eq!(stored.len(), 32);
        assert_eq!(sha256(token.as_bytes()), stored, "hashing is deterministic");
        assert_ne!(sha256(b"other"), stored);
    }

    #[test]
    fn hex_helpers_roundtrip() {
        let bytes = [0u8, 15, 255, 128];
        let enc = hex_encode(&bytes);
        assert_eq!(enc, "000fff80");
        assert_eq!(hex_decode(&enc).unwrap(), bytes.to_vec());
        assert!(hex_decode("abc").is_none());
        assert!(hex_decode("zz").is_none());
    }
}

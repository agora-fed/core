//! # Proof of notification — the "digital registered mail of silence" (0.29, migration 0521).
//!
//! Every e-mail to a cabinet becomes a hash-chained receipt per proposal:
//! `hash = sha256(prev|proposal|recipient|attempt|outcome|sent_at)`, with
//! genesis `sha256("genesis:<proposal_id>")`. Tampering with a receipt breaks the
//! the chain from there on — anyone can verify it with sha256 by hand.
//!
//! - Writing: [`record`] is called by `proposal_delivery` (D0) and by the
//!   worker's escalation loop (D+1/D+2, while the SLA is
//!   `pending`, max 3 attempts).
//! - Public read: `GET /proposals/{id}/delivery-receipts` — the auditable
//!   timeline that turns silence from an accusation into a fact.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/proposals/{id}/delivery-receipts", get(list))
        .with_state(state)
}

fn sha256_hex(input: &str) -> String {
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    format!("{:x}", h.finalize())
}

/// Hash of a receipt — a pure function any auditor can reproduce.
fn receipt_hash(
    prev: &str,
    proposal: Uuid,
    recipient: &str,
    attempt: i32,
    outcome: &str,
    sent_at: DateTime<Utc>,
) -> String {
    sha256_hex(&format!(
        "{prev}|{proposal}|{recipient}|{attempt}|{outcome}|{}",
        sent_at.to_rfc3339()
    ))
}

/// Write the next receipt in the proposal's chain. Structural idempotency:
/// the UNIQUE (proposal_id, attempt) makes a repeated attempt a no-op.
/// It never propagates an error — a receipt is audit, it must not break the send.
pub(crate) async fn record(
    db: &PgPool,
    proposal_id: Uuid,
    mandate_id: Option<Uuid>,
    recipient: &str,
    subject: &str,
    outcome: &str,
) {
    let prev: Option<(i32, String)> = sqlx::query_as(
        r"SELECT attempt, hash FROM notification_receipt
           WHERE proposal_id = $1 ORDER BY attempt DESC LIMIT 1",
    )
    .bind(proposal_id)
    .fetch_optional(db)
    .await
    .unwrap_or(None);
    let (attempt, prev_hash) = match prev {
        Some((last, _)) if last >= 3 => {
            tracing::warn!(%proposal_id, last, "receipt chain full; not recording");
            return;
        }
        Some((last, hash)) => (last + 1, hash),
        None => (1, sha256_hex(&format!("genesis:{proposal_id}"))),
    };
    let sent_at = Utc::now();
    let hash = receipt_hash(
        &prev_hash,
        proposal_id,
        recipient,
        attempt,
        outcome,
        sent_at,
    );
    let res = sqlx::query(
        r"INSERT INTO notification_receipt
              (id, proposal_id, mandate_id, recipient, attempt, subject, outcome,
               sent_at, prev_hash, hash)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
          ON CONFLICT (proposal_id, attempt) DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(proposal_id)
    .bind(mandate_id)
    .bind(recipient)
    .bind(attempt)
    .bind(subject)
    .bind(outcome)
    .bind(sent_at)
    .bind(&prev_hash)
    .bind(&hash)
    .execute(db)
    .await;
    match res {
        Ok(_) => tracing::info!(%proposal_id, attempt, outcome, "notification receipt recorded"),
        Err(err) => tracing::error!(?err, %proposal_id, "notification receipt insert failed"),
    }
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct ReceiptDto {
    attempt: i32,
    recipient: String,
    subject: String,
    outcome: String,
    sent_at: DateTime<Utc>,
    prev_hash: String,
    hash: String,
}

/// Public timeline of a proposal's warnings — the heart of the "digital registered mail".
async fn list(State(state): State<AppState>, Path(proposal_id): Path<Uuid>) -> Response {
    let rows: Result<Vec<ReceiptDto>, _> = sqlx::query_as(
        r"SELECT attempt, recipient, subject, outcome, sent_at, prev_hash, hash
            FROM notification_receipt
           WHERE proposal_id = $1
           ORDER BY attempt",
    )
    .bind(proposal_id)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(list) => (StatusCode::OK, Json(ApiResponse::ok(list))).into_response(),
        Err(err) => {
            tracing::error!(?err, "delivery receipts list");
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
    fn receipt_hash_is_deterministic_and_chained() {
        let p = Uuid::nil();
        let t = DateTime::parse_from_rfc3339("2026-07-10T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let genesis = sha256_hex(&format!("genesis:{p}"));
        let h1 = receipt_hash(&genesis, p, "gab@example.leg.br", 1, "accepted", t);
        let h1_again = receipt_hash(&genesis, p, "gab@example.leg.br", 1, "accepted", t);
        assert_eq!(h1, h1_again, "auditor reproduz o mesmo hash");
        let h2 = receipt_hash(&h1, p, "gab@example.leg.br", 2, "accepted", t);
        assert_ne!(h1, h2);
        // Adulterar o outcome do 1º muda h1 — e portanto quebraria h2.
        let tampered = receipt_hash(&genesis, p, "gab@example.leg.br", 1, "failed", t);
        assert_ne!(h1, tampered);
    }
}

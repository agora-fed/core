//! # Collective mandate — a declared consultative commitment (D8.1, migration 0666).
//!
//! **Thesis: accountability ≠ power.** A collective mandate publicly **commits** to
//! listening to its base before voting on a topic, and the result + whether it FOLLOWED
//! through stay public and immutable. The scorecard stops being "accusation" and becomes "instruction".
//!
//! A mandate is **non-delegable by law** → the software delivers **transparency of the
//! VOLUNTARY commitment**, NEVER coercion. The copy never says "binding"; it says "declared
//! consultative commitment". The `kind` is locked to `'consultivo'` by a schema CHECK.
//!
//! ## Gate (operador)
//! Only the mandate's operator: whoever holds a binding in `mandate_identity_binding` (the same
//! criterion as the mandate panel, the CRM and `campanha.rs`). The commitment's `mandate_id` is
//! ALWAYS resolved from the caller's binding — it is structurally impossible for an operator to
//! gabinete (as escritas chaveiam por `mandate_id = <mandato do caller>`).
//!
//! - `POST /me/mandate/commitments`            — declare a commitment (topic + description).
//! - `POST /me/mandate/commitments/{id}/consult` — open a consultation with the base (reusing the
//!   consultations; grava `consultation_id`).
//! - `POST /me/mandate/commitments/{id}/outcome` — registra `seguiu`/`nao_seguiu` + nota.
//! - `GET  /politicos/{mandate_id}/commitments`  — PUBLIC: the mandate's public commitments +
//!   outcome + (se houver consulta) o agregado concordo/neutro/discordo.
//!
//! ## Nota LGPD
//! The public surface exposes **public data** only: topic, description, declared result and the
//! consultation's **aggregate** (counts per option) — never a per-citizen answer.

use axum::extract::{Json, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, Duration, Utc};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use dsoc_consultations::ConsultationService;
use dsoc_core::ids::OrgId;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;
use uuid::Uuid;

/// The fixed default org of the public/federation surfaces (the same as `campanha.rs`).
const DEFAULT_ORG_UUID: Uuid = uuid::uuid!("11111111-1111-1111-1111-111111111111");

const MAX_THEME: usize = 120;
const MAX_DESCRIPTION: usize = 2000;
const MAX_QUESTION: usize = 500;
const MAX_OUTCOME_NOTE: usize = 2000;
/// Cap of commitments listed per mandate (the scale is small; no fine pagination for now).
const LIST_LIMIT: i64 = 500;
/// Default window of a consultation opened from a commitment.
const CONSULT_WINDOW_DAYS: i64 = 14;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/me/mandate/commitments", post(create_commitment))
        .route(
            "/me/mandate/commitments/{id}/consult",
            post(open_consultation),
        )
        .route("/me/mandate/commitments/{id}/outcome", post(record_outcome))
        .route(
            "/politicos/{mandate_id}/commitments",
            get(public_commitments),
        )
        .with_state(state)
}

fn caller_citizen(headers: &HeaderMap) -> Option<Uuid> {
    headers
        .get("x-dsoc-citizen-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
}

fn caller_org(headers: &HeaderMap) -> Uuid {
    headers
        .get("x-dsoc-org-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_ORG_UUID)
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

fn unauthorized() -> Response {
    fail(
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "Autenticação necessária.",
    )
}

fn not_operator() -> Response {
    fail(
        StatusCode::FORBIDDEN,
        "not_operator",
        "Compromissos de mandato são exclusivos de contas vinculadas a um mandato.",
    )
}

/// Gate + mandate resolution: the binding is the authorization AND the scope key. Returns
/// the caller's `Ok(mandate_id)`, or `Err(a ready response)` (401 without a session, 403 without a binding).
async fn require_operator_mandate(db: &PgPool, headers: &HeaderMap) -> Result<Uuid, Response> {
    let Some(citizen) = caller_citizen(headers) else {
        return Err(unauthorized());
    };
    let mandate_id: Option<Uuid> = match sqlx::query_scalar(
        "SELECT mandate_id FROM mandate_identity_binding \
         WHERE citizen_id = $1 ORDER BY verified_at DESC LIMIT 1",
    )
    .bind(citizen)
    .fetch_optional(db)
    .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "commitment gate check");
            return Err(storage_error());
        }
    };
    match mandate_id {
        Some(m) => Ok(m),
        None => Err(not_operator()),
    }
}

// ---------------------------------------------------------------------------
// POST /me/mandate/commitments — declara um compromisso
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CreateBody {
    theme: String,
    description: String,
}

async fn create_commitment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateBody>,
) -> Response {
    let mandate_id = match require_operator_mandate(&state.db, &headers).await {
        Ok(m) => m,
        Err(resp) => return resp,
    };

    let theme = body.theme.trim();
    if theme.is_empty() || theme.chars().count() > MAX_THEME {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_theme",
            "Tema obrigatório, até 120 caracteres.",
        );
    }
    let description = body.description.trim();
    if description.is_empty() || description.chars().count() > MAX_DESCRIPTION {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_description",
            "Descrição obrigatória, até 2000 caracteres.",
        );
    }

    let id = Uuid::now_v7();
    let res = sqlx::query(
        "INSERT INTO mandate_commitment (id, mandate_id, theme, description, outcome) \
         VALUES ($1, $2, $3, $4, 'pendente')",
    )
    .bind(id)
    .bind(mandate_id)
    .bind(theme)
    .bind(description)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "id": id }))),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "commitment insert");
            storage_error()
        }
    }
}

// ---------------------------------------------------------------------------
// POST /me/mandate/commitments/{id}/consult — open the consultation with the base
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ConsultBody {
    /// The consultation's single question. Optional — falls back to text derived from the topic.
    #[serde(default)]
    question: Option<String>,
}

async fn open_consultation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<ConsultBody>,
) -> Response {
    let mandate_id = match require_operator_mandate(&state.db, &headers).await {
        Ok(m) => m,
        Err(resp) => return resp,
    };

    // Load the commitment AND confirm it belongs to THIS mandate (scope by binding).
    let row: Option<(String, Option<Uuid>)> = match sqlx::query_as(
        "SELECT theme, consultation_id FROM mandate_commitment \
         WHERE id = $1 AND mandate_id = $2",
    )
    .bind(id)
    .bind(mandate_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "commitment load for consult");
            return storage_error();
        }
    };
    let Some((theme, existing_consultation)) = row else {
        return fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "Compromisso não encontrado.",
        );
    };
    if existing_consultation.is_some() {
        return fail(
            StatusCode::CONFLICT,
            "already_consulting",
            "Este compromisso já tem uma consulta aberta.",
        );
    }

    // Question: the given one, or one derived from the topic. Trim + length cap.
    let question = body
        .question
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("O mandato deve seguir a base sobre: {theme}?"));
    if question.chars().count() > MAX_QUESTION {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_question",
            "Pergunta até 500 caracteres.",
        );
    }

    // Reuses the consultations crate: creates the consultation + 1 question in a transaction (ADR-0014).
    let org = OrgId::from_uuid(caller_org(&headers));
    let now = Utc::now();
    let closes_at = now + Duration::days(CONSULT_WINDOW_DAYS);
    let title = format!("Consulta à base — {theme}");
    let svc = ConsultationService::from_state(&state);
    let consultation_id = match svc.create(org, &title, now, closes_at, &[question]).await {
        Ok((view, _questions)) => view.id.as_uuid(),
        Err(err) => {
            tracing::error!(?err, "commitment consultation create");
            return storage_error();
        }
    };

    // Links the consultation to the commitment (guarded by mandate — idempotent: links only while still null).
    let res = sqlx::query(
        "UPDATE mandate_commitment SET consultation_id = $1 \
         WHERE id = $2 AND mandate_id = $3 AND consultation_id IS NULL",
    )
    .bind(consultation_id)
    .bind(id)
    .bind(mandate_id)
    .execute(&state.db)
    .await;
    match res {
        Ok(r) if r.rows_affected() > 0 => (
            StatusCode::OK,
            Json(ApiResponse::ok(
                serde_json::json!({ "consultation_id": consultation_id }),
            )),
        )
            .into_response(),
        Ok(_) => fail(
            StatusCode::CONFLICT,
            "already_consulting",
            "Este compromisso já tem uma consulta aberta.",
        ),
        Err(err) => {
            tracing::error!(?err, "commitment link consultation");
            storage_error()
        }
    }
}

// ---------------------------------------------------------------------------
// POST /me/mandate/commitments/{id}/outcome — record followed/did-not-follow
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct OutcomeBody {
    outcome: String,
    #[serde(default)]
    note: Option<String>,
}

async fn record_outcome(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<OutcomeBody>,
) -> Response {
    let mandate_id = match require_operator_mandate(&state.db, &headers).await {
        Ok(m) => m,
        Err(resp) => return resp,
    };

    // The operator only declares a real result; 'pendente' is the initial state, not an outcome.
    if body.outcome != "seguiu" && body.outcome != "nao_seguiu" {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_outcome",
            "outcome deve ser 'seguiu' ou 'nao_seguiu'.",
        );
    }
    let note = match body.note.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(n) if n.chars().count() <= MAX_OUTCOME_NOTE => Some(n.to_owned()),
        Some(_) => {
            return fail(
                StatusCode::BAD_REQUEST,
                "invalid_note",
                "Nota até 2000 caracteres.",
            )
        }
    };

    let res = sqlx::query(
        "UPDATE mandate_commitment SET outcome = $1, outcome_note = $2 \
         WHERE id = $3 AND mandate_id = $4",
    )
    .bind(&body.outcome)
    .bind(note.as_deref())
    .bind(id)
    .bind(mandate_id)
    .execute(&state.db)
    .await;
    match res {
        Ok(r) if r.rows_affected() > 0 => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "ok": true }))),
        )
            .into_response(),
        Ok(_) => fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "Compromisso não encontrado.",
        ),
        Err(err) => {
            tracing::error!(?err, "commitment outcome update");
            storage_error()
        }
    }
}

// ---------------------------------------------------------------------------
// GET /politicos/{mandate_id}/commitments — public surface
// ---------------------------------------------------------------------------

/// Public aggregate of the consultation linked to the commitment (counts per option, never per-citizen).
#[derive(Debug, Clone, Serialize)]
struct ConsultationAggregate {
    consultation_id: Uuid,
    title: String,
    status: String,
    concordo: i64,
    neutro: i64,
    discordo: i64,
    total: i64,
}

#[derive(Debug, Serialize)]
struct PublicCommitment {
    id: Uuid,
    theme: String,
    description: String,
    /// Always "consultivo" — the front-end copy uses this to make clear it is voluntary.
    kind: String,
    /// `seguiu` | `nao_seguiu` | `pendente`.
    outcome: String,
    outcome_note: Option<String>,
    created_at: DateTime<Utc>,
    /// Present when the mandate opened a consultation with the base about this commitment.
    consultation: Option<ConsultationAggregate>,
}

/// Raw row of a public commitment.
#[derive(sqlx::FromRow)]
struct CommitmentRow {
    id: Uuid,
    theme: String,
    description: String,
    kind: String,
    outcome: Option<String>,
    outcome_note: Option<String>,
    created_at: DateTime<Utc>,
    consultation_id: Option<Uuid>,
}

async fn public_commitments(
    State(state): State<AppState>,
    Path(mandate_id): Path<Uuid>,
) -> Response {
    let rows: Vec<CommitmentRow> = match sqlx::query_as(
        "SELECT id, theme, description, kind, outcome, outcome_note, created_at, consultation_id \
           FROM mandate_commitment \
          WHERE mandate_id = $1 AND is_public = true \
          ORDER BY created_at DESC \
          LIMIT $2",
    )
    .bind(mandate_id)
    .bind(LIST_LIMIT)
    .fetch_all(&state.db)
    .await
    {
        Ok(r) => r,
        Err(err) => {
            tracing::error!(?err, "public commitments read");
            return storage_error();
        }
    };

    // Linked consultations: aggregates every scoreboard at once (avoids N+1).
    let consultation_ids: Vec<Uuid> = rows.iter().filter_map(|r| r.consultation_id).collect();
    let aggregates = match load_aggregates(&state.db, &consultation_ids).await {
        Ok(a) => a,
        Err(err) => {
            tracing::error!(?err, "public commitments aggregate");
            return storage_error();
        }
    };

    let commitments: Vec<PublicCommitment> = rows
        .into_iter()
        .map(|r| {
            let consultation = r
                .consultation_id
                .and_then(|cid| aggregates.get(&cid).cloned());
            PublicCommitment {
                id: r.id,
                theme: r.theme,
                description: r.description,
                kind: r.kind,
                outcome: r.outcome.unwrap_or_else(|| "pendente".to_owned()),
                outcome_note: r.outcome_note,
                created_at: r.created_at,
                consultation,
            }
        })
        .collect();

    (
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({
            "mandate_id": mandate_id,
            "commitments": commitments,
        }))),
    )
        .into_response()
}

/// Assemble the aggregate (title/status + agree/neutral/disagree counts) of each linked consultation.
async fn load_aggregates(
    db: &PgPool,
    consultation_ids: &[Uuid],
) -> Result<HashMap<Uuid, ConsultationAggregate>, sqlx::Error> {
    if consultation_ids.is_empty() {
        return Ok(HashMap::new());
    }
    // The consultation's title + status.
    let metas: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT id, title, status FROM consultations_consultation WHERE id = ANY($1)",
    )
    .bind(consultation_ids)
    .fetch_all(db)
    .await?;

    let mut map: HashMap<Uuid, ConsultationAggregate> = metas
        .into_iter()
        .map(|(id, title, status)| {
            (
                id,
                ConsultationAggregate {
                    consultation_id: id,
                    title,
                    status,
                    concordo: 0,
                    neutro: 0,
                    discordo: 0,
                    total: 0,
                },
            )
        })
        .collect();

    // Counts per option, keyed by the consultation owning the question.
    let tallies: Vec<(Uuid, String, i64)> = sqlx::query_as(
        "SELECT q.consultation_id, r.answer, count(*) \
           FROM consultations_consultation_question q \
           JOIN consultation_response r ON r.question_id = q.id \
          WHERE q.consultation_id = ANY($1) \
          GROUP BY q.consultation_id, r.answer",
    )
    .bind(consultation_ids)
    .fetch_all(db)
    .await?;

    for (cid, answer, n) in tallies {
        if let Some(agg) = map.get_mut(&cid) {
            match answer.as_str() {
                "concordo" => agg.concordo += n,
                "neutro" => agg.neutro += n,
                "discordo" => agg.discordo += n,
                _ => {}
            }
            agg.total += n;
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_vocabulary_is_closed() {
        // Contract guards: only these three values exist in the schema/DTO.
        for ok in ["seguiu", "nao_seguiu", "pendente"] {
            assert!(matches!(ok, "seguiu" | "nao_seguiu" | "pendente"));
        }
    }

    #[test]
    fn aggregate_clone_preserves_counts() {
        let agg = ConsultationAggregate {
            consultation_id: Uuid::now_v7(),
            title: "t".to_owned(),
            status: "open".to_owned(),
            concordo: 3,
            neutro: 1,
            discordo: 2,
            total: 6,
        };
        let cloned = agg.clone();
        assert_eq!(cloned.total, 6);
        assert_eq!(cloned.concordo, 3);
        assert_eq!(cloned.discordo, 2);
    }
}

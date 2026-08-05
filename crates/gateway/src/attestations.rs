//! # Citizenship attestation by a verified operator (0.28.3, migration 0519).
//!
//! Web of trust: while there is no institutional verification (TSE/gov.br),
//! whoever ALREADY holds a strong identity on the platform — a mandate operator
//! (`mandate_identity_binding`) or an accepted party admin
//! (`party_administrator`) — may publicly attest that they know a
//! citizen. The attestation is auditable (who, when, with what power) and
//! revocable by the attester themselves; the badge shows on the public profile.
//!
//! - `GET    /citizens/{id}/attestations` — public; with a session it includes
//!   `viewer_can_attest`/`viewer_attested` for the UI.
//! - `POST   /citizens/{id}/attestations {note?}` — attest (or revive a
//!   revoked attestation for the same pair).
//! - `DELETE /citizens/{id}/attestations` — revoke one's own attestation.

use axum::extract::{Json, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use chrono::{DateTime, Utc};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

const MAX_NOTE: usize = 280;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route(
            "/citizens/{id}/attestations",
            get(list).post(attest).delete(revoke),
        )
        .with_state(state)
}

fn caller_citizen(headers: &HeaderMap) -> Option<Uuid> {
    headers
        .get("x-dsoc-citizen-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
}

fn fail(status: StatusCode, code: &str, msg: &str) -> Response {
    (status, Json(ApiResponse::<()>::fail(code, msg))).into_response()
}

/// With what power may the citizen attest? A mandate operator outranks
/// a party admin when they hold both (a mandate is the stronger binding).
async fn attester_kind(db: &PgPool, citizen: Uuid) -> Result<Option<&'static str>, sqlx::Error> {
    let is_operator: bool = sqlx::query_scalar(
        r"SELECT EXISTS (SELECT 1 FROM mandate_identity_binding WHERE citizen_id = $1)",
    )
    .bind(citizen)
    .fetch_one(db)
    .await?;
    if is_operator {
        return Ok(Some("mandato"));
    }
    let is_party_admin: bool = sqlx::query_scalar(
        r"SELECT EXISTS (SELECT 1 FROM party_administrator
                          WHERE citizen_id = $1 AND accepted_at IS NOT NULL)",
    )
    .bind(citizen)
    .fetch_one(db)
    .await?;
    Ok(is_party_admin.then_some("partido"))
}

// ---------------------------------------------------------------------------
// GET — public list + viewer flags
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, sqlx::FromRow)]
struct AttestationItemDto {
    attester_citizen_id: Uuid,
    display_name: Option<String>,
    handle: Option<String>,
    kind: String,
    note: Option<String>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct AttestationsDto {
    count: i64,
    viewer_can_attest: bool,
    viewer_attested: bool,
    items: Vec<AttestationItemDto>,
}

async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(citizen_id): Path<Uuid>,
) -> Response {
    let items: Vec<AttestationItemDto> = match sqlx::query_as(
        r"SELECT a.attester_citizen_id,
                 c.display_name,
                 c.handle,
                 a.attester_kind AS kind,
                 a.note,
                 a.created_at
            FROM citizen_attestation a
            JOIN citizen c ON c.id = a.attester_citizen_id
           WHERE a.citizen_id = $1 AND a.revoked_at IS NULL
           ORDER BY a.created_at DESC
           LIMIT 100",
    )
    .bind(citizen_id)
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(err) => {
            tracing::error!(?err, "attestations list");
            return fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage_error",
                "Erro interno.",
            );
        }
    };

    let viewer = caller_citizen(&headers);
    let (mut viewer_can_attest, mut viewer_attested) = (false, false);
    if let Some(viewer) = viewer {
        if viewer != citizen_id {
            viewer_can_attest = attester_kind(&state.db, viewer)
                .await
                .ok()
                .flatten()
                .is_some();
        }
        viewer_attested = items.iter().any(|i| i.attester_citizen_id == viewer);
    }

    let count = items.len() as i64;
    (
        StatusCode::OK,
        Json(ApiResponse::ok(AttestationsDto {
            count,
            viewer_can_attest,
            viewer_attested,
            items,
        })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// POST — attest (or revive a revoked attestation for the same pair)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct AttestBody {
    #[serde(default)]
    note: Option<String>,
}

async fn attest(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(citizen_id): Path<Uuid>,
    Json(body): Json<AttestBody>,
) -> Response {
    let Some(attester) = caller_citizen(&headers) else {
        return fail(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Autenticação necessária.",
        );
    };
    if attester == citizen_id {
        return fail(
            StatusCode::UNPROCESSABLE_ENTITY,
            "self_attest",
            "Não é possível atestar a própria conta.",
        );
    }
    let note = match body.note.as_deref().map(str::trim) {
        Some(n) if n.len() > MAX_NOTE => {
            return fail(
                StatusCode::BAD_REQUEST,
                "note_too_long",
                "A nota do atestado tem limite de 280 caracteres.",
            );
        }
        Some(n) if !n.is_empty() => Some(n.to_owned()),
        _ => None,
    };
    let kind = match attester_kind(&state.db, attester).await {
        Ok(Some(k)) => k,
        Ok(None) => {
            return fail(
                StatusCode::FORBIDDEN,
                "no_attest_power",
                "Só operadores de mandato ou administradores de partido podem atestar.",
            );
        }
        Err(err) => {
            tracing::error!(?err, "attestations power check");
            return fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage_error",
                "Erro interno.",
            );
        }
    };
    let target_exists: bool = sqlx::query_scalar(
        r"SELECT EXISTS (SELECT 1 FROM citizen WHERE id = $1 AND suspended_at IS NULL)",
    )
    .bind(citizen_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(false);
    if !target_exists {
        return fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "Cidadão não encontrado.",
        );
    }
    let res = sqlx::query(
        r"INSERT INTO citizen_attestation
              (id, citizen_id, attester_citizen_id, attester_kind, note)
          VALUES ($1, $2, $3, $4, $5)
          ON CONFLICT (citizen_id, attester_citizen_id)
          DO UPDATE SET revoked_at = NULL,
                        attester_kind = EXCLUDED.attester_kind,
                        note = EXCLUDED.note",
    )
    .bind(Uuid::now_v7())
    .bind(citizen_id)
    .bind(attester)
    .bind(kind)
    .bind(note.as_deref())
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(
                serde_json::json!({ "ok": true, "kind": kind }),
            )),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "attestations insert");
            fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage_error",
                "Erro interno.",
            )
        }
    }
}

// ---------------------------------------------------------------------------
// DELETE — revoke one's own attestation
// ---------------------------------------------------------------------------

async fn revoke(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(citizen_id): Path<Uuid>,
) -> Response {
    let Some(attester) = caller_citizen(&headers) else {
        return fail(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Autenticação necessária.",
        );
    };
    let res = sqlx::query(
        r"UPDATE citizen_attestation
             SET revoked_at = now()
           WHERE citizen_id = $1
             AND attester_citizen_id = $2
             AND revoked_at IS NULL",
    )
    .bind(citizen_id)
    .bind(attester)
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
            "Você não tem atestado ativo para esta conta.",
        ),
        Err(err) => {
            tracing::error!(?err, "attestations revoke");
            fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage_error",
                "Erro interno.",
            )
        }
    }
}

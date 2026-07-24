//! # Elections + candidacies — public comparador for 2026 (0.21.0-alpha).
//!
//! Read-only endpoints backing `/eleicoes/2026`:
//!
//! * `GET /api/v1/elections`                    — every election on record.
//! * `GET /api/v1/elections/{id}/candidacies?…` — every candidacy in one
//!   election, filterable by UF / office / party sigla / gender.
//! * `GET /api/v1/candidacies/{id}`             — one candidate detail.
//!
//! Data is loaded via a separate script (TSE CSV → migration data); this
//! module just serves what's in the tables. Every query is runtime-checked
//! `sqlx` to keep the offline cache stable, mirroring `admin_ext.rs`.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, NaiveDate, Utc};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/elections", get(list_elections))
        .route("/elections/{id}/candidacies", get(list_candidacies))
        .route("/candidacies/{id}", get(get_candidacy))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn server_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiResponse::<()>::fail("http_500", "Erro interno.")),
    )
        .into_response()
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(ApiResponse::<()>::fail(
            "http_404",
            "Candidatura não encontrada.",
        )),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct ElectionDto {
    id: Uuid,
    year: i32,
    round: i32,
    sphere: String,
    election_day: NaiveDate,
    registration_deadline: Option<NaiveDate>,
    candidacy_count: i64,
}

#[derive(Debug, Serialize)]
struct CandidacyDto {
    id: Uuid,
    election_id: Uuid,
    mandate_id: Option<Uuid>,
    candidate_name: String,
    candidate_gender: Option<String>,
    party_sigla: String,
    office: String,
    number: String,
    sphere_uf: Option<String>,
    sphere_municipio: Option<String>,
    result_rank: Option<i32>,
    status: Option<String>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Default, Deserialize)]
struct CandidacyFilters {
    #[serde(default)]
    uf: Option<String>,
    #[serde(default)]
    office: Option<String>,
    #[serde(default)]
    party: Option<String>,
    #[serde(default)]
    gender: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    offset: Option<i64>,
}

// ---------------------------------------------------------------------------
// GET /elections
// ---------------------------------------------------------------------------

async fn list_elections(State(state): State<AppState>) -> Response {
    let rows: Result<Vec<(Uuid, i32, i32, String, NaiveDate, Option<NaiveDate>, i64)>, _> =
        sqlx::query_as(
            r"SELECT e.id, e.year, e.round, e.sphere, e.election_day,
                 e.registration_deadline,
                 (SELECT count(*) FROM candidacy c WHERE c.election_id = e.id) AS candidacy_count
            FROM election e
           ORDER BY e.year DESC, e.round ASC, e.sphere",
        )
        .fetch_all(&state.db)
        .await;
    match rows {
        Ok(rows) => (
            StatusCode::OK,
            Json(ApiResponse::ok(
                rows.into_iter()
                    .map(|r| ElectionDto {
                        id: r.0,
                        year: r.1,
                        round: r.2,
                        sphere: r.3,
                        election_day: r.4,
                        registration_deadline: r.5,
                        candidacy_count: r.6,
                    })
                    .collect::<Vec<_>>(),
            )),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "list_elections sql");
            server_error()
        }
    }
}

// ---------------------------------------------------------------------------
// GET /elections/{id}/candidacies?uf=&office=&party=&gender=&q=&limit=&offset=
// ---------------------------------------------------------------------------

async fn list_candidacies(
    State(state): State<AppState>,
    Path(election_id): Path<Uuid>,
    Query(f): Query<CandidacyFilters>,
) -> Response {
    let limit = f.limit.unwrap_or(50).clamp(1, 500);
    let offset = f.offset.unwrap_or(0).max(0);
    let name_pat =
        f.q.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| format!("%{s}%"));
    let rows: Result<
        Vec<(
            Uuid,
            Uuid,
            Option<Uuid>,
            String,
            Option<String>,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<i32>,
            Option<String>,
            DateTime<Utc>,
        )>,
        _,
    > = sqlx::query_as(
        r"SELECT id, election_id, mandate_id, candidate_name, candidate_gender,
                 party_sigla, office, number,
                 sphere_uf, sphere_municipio, result_rank, status, created_at
            FROM candidacy
           WHERE election_id = $1
             AND listed
             AND ($2::text IS NULL OR sphere_uf = $2)
             AND ($3::text IS NULL OR office = $3)
             AND ($4::text IS NULL OR party_sigla = $4)
             AND ($5::text IS NULL OR candidate_gender = $5)
             AND ($6::text IS NULL OR candidate_name ILIKE $6)
           ORDER BY party_sigla, candidate_name
           LIMIT $7 OFFSET $8",
    )
    .bind(election_id)
    .bind(f.uf.as_deref())
    .bind(f.office.as_deref())
    .bind(f.party.as_deref())
    .bind(f.gender.as_deref())
    .bind(name_pat.as_deref())
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => (
            StatusCode::OK,
            Json(ApiResponse::ok(
                rows.into_iter()
                    .map(|r| CandidacyDto {
                        id: r.0,
                        election_id: r.1,
                        mandate_id: r.2,
                        candidate_name: r.3,
                        candidate_gender: r.4,
                        party_sigla: r.5,
                        office: r.6,
                        number: r.7,
                        sphere_uf: r.8,
                        sphere_municipio: r.9,
                        result_rank: r.10,
                        status: r.11,
                        created_at: r.12,
                    })
                    .collect::<Vec<_>>(),
            )),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "list_candidacies sql");
            server_error()
        }
    }
}

// ---------------------------------------------------------------------------
// GET /candidacies/{id}
// ---------------------------------------------------------------------------

async fn get_candidacy(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    let row: Result<
        Option<(
            Uuid,
            Uuid,
            Option<Uuid>,
            String,
            Option<String>,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<i32>,
            Option<String>,
            DateTime<Utc>,
        )>,
        _,
    > = sqlx::query_as(
        r"SELECT id, election_id, mandate_id, candidate_name, candidate_gender,
                 party_sigla, office, number,
                 sphere_uf, sphere_municipio, result_rank, status, created_at
            FROM candidacy
           WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await;
    match row {
        Ok(Some(r)) => (
            StatusCode::OK,
            Json(ApiResponse::ok(CandidacyDto {
                id: r.0,
                election_id: r.1,
                mandate_id: r.2,
                candidate_name: r.3,
                candidate_gender: r.4,
                party_sigla: r.5,
                office: r.6,
                number: r.7,
                sphere_uf: r.8,
                sphere_municipio: r.9,
                result_rank: r.10,
                status: r.11,
                created_at: r.12,
            })),
        )
            .into_response(),
        Ok(None) => not_found(),
        Err(err) => {
            tracing::error!(?err, "get_candidacy sql");
            server_error()
        }
    }
}

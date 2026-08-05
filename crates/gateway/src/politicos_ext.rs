//! # Filtered politicos browser (0.23.0-municipais).
//!
//! With ~68k vereadores + ~11k prefeitos landing in `mandate`, the old client
//! that pulled every row (~700 round-trips) is unusable. This module adds
//! server-side filtered browsing and a municipality dropdown that only lists
//! municipios which actually have a mandate cadastrado.
//!
//! * `GET /api/v1/politicos/browse?sphere=&uf=&municipio=&q=&limit=&offset=`
//!   Returns a page of mandates. The frontend enforces "you MUST pick sphere
//!   (and UF/municipio when applicable) before we query" so the payload stays
//!   small (default cap 200 rows / call).
//! * `GET /api/v1/politicos/municipios?uf=X` — distinct list of municipios
//!   inside a UF that have at least one municipal mandate.
//!
//! Runtime-checked sqlx (same pattern as `admin_ext.rs`).

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Every query scopes to this tenant — mirrors the pattern in `admin_ext.rs`.
const DEFAULT_ORG_UUID: Uuid = uuid::uuid!("11111111-1111-1111-1111-111111111111");

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/politicos/browse", get(browse))
        .route("/politicos/municipios", get(municipios))
        // Resumo territorial (0.38.0 — Fase 2.2): eleitorado + mandatos por
        // a municipality's party, for the /municipio page.
        .route("/politicos/territorio", get(territorio))
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

fn bad_request(msg: &'static str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ApiResponse::<()>::fail("http_400", msg)),
    )
        .into_response()
}

fn media_base() -> String {
    // MEDIA_BASE_URL points at the public origin of the MinIO bucket. Same
    // env var used everywhere else in the gateway.
    std::env::var("MEDIA_BASE_URL")
        .unwrap_or_else(|_| "https://democracia.social.br/media".to_owned())
}

fn resolve_avatar(object_key: Option<&str>) -> Option<String> {
    let key = object_key?.trim();
    if key.is_empty() {
        return None;
    }
    Some(format!("{}/{}", media_base().trim_end_matches('/'), key))
}

// ---------------------------------------------------------------------------
// GET /politicos/browse
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct BrowseParams {
    #[serde(default)]
    sphere: Option<String>,
    #[serde(default)]
    uf: Option<String>,
    #[serde(default)]
    municipio: Option<String>,
    #[serde(default)]
    party: Option<String>,
    #[serde(default)]
    house: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    offset: Option<i64>,
}

#[derive(Debug, Serialize)]
struct BrowseRow {
    id: Uuid,
    display_name: String,
    office: String,
    party: Option<String>,
    uf: Option<String>,
    municipio: Option<String>,
    house: Option<String>,
    sphere: String,
    avatar_url: Option<String>,
    is_candidate: bool,
    has_verified_operator: bool,
}

#[derive(Debug, Serialize)]
struct BrowseResponse {
    total: i64,
    limit: i64,
    offset: i64,
    items: Vec<BrowseRow>,
}

async fn browse(State(state): State<AppState>, Query(p): Query<BrowseParams>) -> Response {
    // Rule 1 — sphere is REQUIRED. Without it the caller would drag every
    // 70k+ row through the network. The UI enforces this too; we double-check
    // here to protect the server.
    let sphere = match p.sphere.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) if matches!(s, "federal" | "estadual" | "municipal") => s.to_owned(),
        _ => return bad_request("Escolha uma esfera (federal, estadual ou municipal)."),
    };
    // Rule 2 — for municipal, both UF and municipality are mandatory (there
    // are ~5.5k municipalities; without one the response can be 500+
    // rows in some capitals but still bounded).
    let uf_clean =
        p.uf.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty() && s.len() == 2)
            .map(str::to_ascii_uppercase);
    let municipio_clean = p
        .municipio
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    if sphere == "municipal" {
        if uf_clean.is_none() {
            return bad_request("Escolha um estado para ver políticos municipais.");
        }
        if municipio_clean.is_none() {
            return bad_request("Escolha um município para ver políticos municipais.");
        }
    }
    let party_clean = p
        .party
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let house_clean = p
        .house
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let q_pat =
        p.q.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| format!("%{s}%"));
    let limit = p.limit.unwrap_or(200).clamp(1, 500);
    let offset = p.offset.unwrap_or(0).max(0);

    // total count first (drives paging in the UI).
    let total: i64 = match sqlx::query_scalar::<_, i64>(
        r"SELECT count(*) FROM mandate
           WHERE org_id = $1
             AND hidden_at IS NULL
             AND sphere = $2
             AND ($3::text IS NULL OR uf = $3)
             AND ($4::text IS NULL OR municipio = $4)
             AND ($5::text IS NULL OR party = $5)
             AND ($6::text IS NULL OR house = $6)
             AND ($7::text IS NULL OR display_name ILIKE $7)",
    )
    .bind(DEFAULT_ORG_UUID)
    .bind(&sphere)
    .bind(uf_clean.as_deref())
    .bind(municipio_clean.as_deref())
    .bind(party_clean.as_deref())
    .bind(house_clean.as_deref())
    .bind(q_pat.as_deref())
    .fetch_one(&state.db)
    .await
    {
        Ok(n) => n,
        Err(err) => {
            tracing::error!(?err, "politicos_ext browse count");
            return server_error();
        }
    };

    let rows: Result<
        Vec<(
            Uuid,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            String,
            Option<String>,
            bool,
            bool,
        )>,
        _,
    > = sqlx::query_as(
        r"SELECT m.id,
                 m.display_name,
                 m.office,
                 m.party,
                 m.uf,
                 m.municipio,
                 m.house,
                 m.sphere,
                 m.avatar_object_key,
                 m.is_candidate,
                 EXISTS(
                    SELECT 1 FROM mandate_identity_binding mib
                     WHERE mib.mandate_id = m.id AND mib.citizen_id IS NOT NULL
                 ) AS has_verified_operator
            FROM mandate m
           WHERE m.org_id = $1
             AND m.hidden_at IS NULL
             AND m.sphere = $2
             AND ($3::text IS NULL OR m.uf = $3)
             AND ($4::text IS NULL OR m.municipio = $4)
             AND ($5::text IS NULL OR m.party = $5)
             AND ($6::text IS NULL OR m.house = $6)
             AND ($7::text IS NULL OR m.display_name ILIKE $7)
           ORDER BY m.display_name
           LIMIT $8 OFFSET $9",
    )
    .bind(DEFAULT_ORG_UUID)
    .bind(&sphere)
    .bind(uf_clean.as_deref())
    .bind(municipio_clean.as_deref())
    .bind(party_clean.as_deref())
    .bind(house_clean.as_deref())
    .bind(q_pat.as_deref())
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await;
    let rows = match rows {
        Ok(rows) => rows,
        Err(err) => {
            tracing::error!(?err, "politicos_ext browse list");
            return server_error();
        }
    };
    let items: Vec<BrowseRow> = rows
        .into_iter()
        .map(
            |(
                id,
                display_name,
                office,
                party,
                uf,
                municipio,
                house,
                sphere,
                avatar_key,
                is_candidate,
                has_verified_operator,
            )| {
                BrowseRow {
                    id,
                    display_name,
                    office,
                    party,
                    uf,
                    municipio,
                    house,
                    sphere,
                    avatar_url: resolve_avatar(avatar_key.as_deref()),
                    is_candidate,
                    has_verified_operator,
                }
            },
        )
        .collect();
    (
        StatusCode::OK,
        Json(ApiResponse::ok(BrowseResponse {
            total,
            limit,
            offset,
            items,
        })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /politicos/municipios
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct MunicipiosParams {
    uf: Option<String>,
}

#[derive(Debug, Serialize)]
struct MunicipioRow {
    nome: String,
    count: i64,
}

async fn municipios(State(state): State<AppState>, Query(p): Query<MunicipiosParams>) -> Response {
    let uf = match p.uf.as_deref().map(str::trim).filter(|s| s.len() == 2) {
        Some(s) => s.to_ascii_uppercase(),
        None => return bad_request("Informe o parâmetro `uf` (UF de 2 letras)."),
    };
    // Only include municipios that actually have a municipal mandate.
    let rows: Result<Vec<(String, i64)>, _> = sqlx::query_as(
        r"SELECT municipio, count(*) AS n
            FROM mandate
           WHERE org_id = $1
             AND sphere = 'municipal'
             AND uf = $2
             AND municipio IS NOT NULL
           GROUP BY municipio
           ORDER BY municipio",
    )
    .bind(DEFAULT_ORG_UUID)
    .bind(&uf)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => {
            let items: Vec<MunicipioRow> = rows
                .into_iter()
                .map(|(nome, count)| MunicipioRow { nome, count })
                .collect();
            (StatusCode::OK, Json(ApiResponse::ok(items))).into_response()
        }
        Err(err) => {
            tracing::error!(?err, "politicos_ext municipios");
            server_error()
        }
    }
}

// ---------------------------------------------------------------------------
// GET /politicos/territorio — a municipality's summary (phase 2.2)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct TerritorioParams {
    uf: Option<String>,
    municipio: Option<String>,
}

#[derive(Debug, Serialize)]
struct PartyCountRow {
    party: String,
    count: i64,
}

#[derive(Debug, Serialize)]
struct TerritorioResponse {
    uf: String,
    municipio: String,
    /// The municipality's official electorate (TSE); `None` when not seeded.
    voters: Option<i64>,
    /// Total municipal mandates in the municipality.
    total: i64,
    /// Mandates per party, largest first.
    by_party: Vec<PartyCountRow>,
}

async fn territorio(State(state): State<AppState>, Query(p): Query<TerritorioParams>) -> Response {
    let uf = match p.uf.as_deref().map(str::trim).filter(|s| s.len() == 2) {
        Some(s) => s.to_ascii_uppercase(),
        None => return bad_request("Informe `uf` (2 letras)."),
    };
    let municipio = match p
        .municipio
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(s) => s.to_owned(),
        None => return bad_request("Informe `municipio`."),
    };

    // Electorate (nullable — not every municipality was seeded).
    let voters: Option<i64> = match sqlx::query_scalar::<_, i64>(
        r"SELECT voters FROM electorate WHERE uf = $1 AND municipio = $2",
    )
    .bind(&uf)
    .bind(&municipio)
    .fetch_optional(&state.db)
    .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "territorio: electorate");
            return server_error();
        }
    };

    // Mandates per party in the municipality.
    let by_party: Vec<(Option<String>, i64)> = match sqlx::query_as(
        r"SELECT party, count(*) AS n
            FROM mandate
           WHERE org_id = $1 AND hidden_at IS NULL
             AND sphere = 'municipal' AND uf = $2 AND municipio = $3
           GROUP BY party
           ORDER BY n DESC, party ASC NULLS LAST",
    )
    .bind(DEFAULT_ORG_UUID)
    .bind(&uf)
    .bind(&municipio)
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(err) => {
            tracing::error!(?err, "territorio: by_party");
            return server_error();
        }
    };

    let total: i64 = by_party.iter().map(|(_, n)| n).sum();
    let by_party = by_party
        .into_iter()
        .filter_map(|(party, count)| party.map(|p| PartyCountRow { party: p, count }))
        .collect();

    (
        StatusCode::OK,
        Json(ApiResponse::ok(TerritorioResponse {
            uf,
            municipio,
            voters,
            total,
            by_party,
        })),
    )
        .into_response()
}

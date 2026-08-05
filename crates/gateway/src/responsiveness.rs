//! # Mandate responsiveness — the official's positive SHOWCASE (Block C of the plan).
//!
//! Today the scorecard offers only a THREAT (silence becomes a public record). This module inverts
//! the reputation economy: from the SAME `answered`/`ignored` counters and the latency that
//! `dsoc-scorecard` already projects (we do NOT reinvent the ledger), we derive a badge/tier, the
//! "answers in ~N days", the streak of answers and the peer comparison — the POSITIVE reason for an
//! vereador/deputado QUERER reivindicar e usar o placar.
//!
//! * `GET /politicos/{mandate_id}/responsiveness` — C1 (badge + streak) + C2 (comparison with peers
//!   of the same level/UF) in a single payload, ready for the official's public page.
//! * `GET /politicos/responsiveness/peers?sphere=&uf=&house=&party=` — C2 standalone: aggregates of
//!   a slice (mean response rate, median latency, group size).
//!
//! The decision logic (tier/percentile) is PURE and tested in `dsoc_scorecard::tier`; here we only
//! query (runtime sqlx, no `.sqlx` — the same pattern as `politicos_ext.rs`/`og_cards.rs`) and
//! assemble the DTO. LGPD: everything exposed is already public (the scorecard IS the public accountability artifact).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use dsoc_scorecard::domain::{median_hours, Outcome};
use dsoc_scorecard::tier;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route(
            "/politicos/{mandate_id}/responsiveness",
            get(mandate_responsiveness),
        )
        .route("/politicos/responsiveness/peers", get(peers_aggregate))
        .with_state(state)
}

fn server_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiResponse::<()>::fail("http_500", "Erro interno.")),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// C1 + C2 — GET /politicos/{mandate_id}/responsiveness
// ---------------------------------------------------------------------------

/// The derived badge + the numbers backing it.
#[derive(Debug, Serialize)]
struct TierDto {
    /// Stable token (unrated|building|bronze|silver|gold) — matches the CSS/badge on the front end.
    key: String,
    /// pt-BR label ("Ouro", "Prata"…).
    label: String,
    /// Medalha (emoji).
    medal: String,
    /// One-line explanation.
    blurb: String,
}

/// Comparison with peers of the same level/UF ("you answered 78% · the RS average is 21%").
#[derive(Debug, Serialize)]
struct PeerComparisonDto {
    /// Label of the compared slice (e.g. "RS", or the level when the UF is unknown).
    scope: String,
    /// How many comparable peers (with at least one demand) exist in the slice.
    peer_count: i64,
    /// Mean response rate of the peers (0–100).
    peer_avg_rate: Option<u32>,
    /// % of peers this mandate beats.
    better_than_pct: Option<u32>,
    /// "Top Y%" — complemento do anterior (menor = melhor).
    top_pct: Option<u32>,
}

/// Public payload of a mandate's responsiveness.
#[derive(Debug, Serialize)]
struct ResponsivenessDto {
    mandate_id: Uuid,
    display_name: String,
    office: String,
    party: Option<String>,
    uf: Option<String>,
    house: Option<String>,
    /// Demandas respondidas dentro do prazo.
    answered: i64,
    /// Ignored demands (public silence).
    ignored: i64,
    /// Response rate 0–100 (None when there are no demands).
    response_rate: Option<u32>,
    /// Median response latency, in hours (None when nothing was answered).
    median_response_hours: Option<f64>,
    /// "Answers in ~N days" (None when nothing was answered).
    responds_in_days: Option<f64>,
    /// Most recent consecutive answers (the consistency medal).
    answer_streak: u32,
    /// O selo/tier.
    tier: TierDto,
    /// Peer comparison.
    peer: PeerComparisonDto,
}

/// Mandate row + scorecard counters (LEFT JOIN: a mandate without a scorecard still counts 0/0).
type MandateScorecardRow = (
    String,         // display_name
    String,         // office
    Option<String>, // party
    Option<String>, // uf
    Option<String>, // sphere
    Option<String>, // house
    i64,            // answered
    i64,            // ignored
    Option<Uuid>,   // scorecard_id
);

async fn mandate_responsiveness(
    State(state): State<AppState>,
    Path(mandate_id): Path<Uuid>,
) -> Response {
    let row: Option<MandateScorecardRow> = match sqlx::query_as(
        r"SELECT m.display_name,
                 m.office,
                 m.party,
                 m.uf,
                 m.sphere,
                 m.house,
                 COALESCE(s.answered, 0),
                 COALESCE(s.ignored, 0),
                 s.id
            FROM mandate m
            LEFT JOIN scorecard s ON s.mandate_id = m.id
           WHERE m.id = $1 AND m.hidden_at IS NULL",
    )
    .bind(mandate_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(r) => r,
        Err(err) => {
            tracing::error!(?err, "responsiveness: mandate lookup");
            return server_error();
        }
    };
    let Some((display_name, office, party, uf, sphere, house, answered, ignored, scorecard_id)) =
        row
    else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::fail(
                "http_404",
                "Político não encontrado.",
            )),
        )
            .into_response();
    };

    // Median latency + streak come from the ledger (scorecard_entry), when a scorecard exists.
    let (median, streak) = match scorecard_id {
        Some(sid) => match load_entries(&state, sid).await {
            Ok((med, strk)) => (med, strk),
            Err(()) => return server_error(),
        },
        None => (None, 0),
    };

    let response_rate = tier::response_rate_pct(answered, ignored);
    let tier_val = tier::responsiveness_tier(answered, ignored, median);

    // Peer comparison: same level (sphere) + same UF, excluding this mandate.
    let peer = match load_peer_comparison(
        &state,
        mandate_id,
        sphere.as_deref(),
        uf.as_deref(),
        response_rate,
    )
    .await
    {
        Ok(p) => p,
        Err(()) => return server_error(),
    };

    let dto = ResponsivenessDto {
        mandate_id,
        display_name,
        office,
        party,
        uf,
        house,
        answered,
        ignored,
        response_rate,
        median_response_hours: median,
        responds_in_days: tier::responds_in_days(median),
        answer_streak: streak,
        tier: TierDto {
            key: tier_val.key().to_owned(),
            label: tier_val.label().to_owned(),
            medal: tier_val.medal().to_owned(),
            blurb: tier_val.blurb().to_owned(),
        },
        peer,
    };
    (StatusCode::OK, Json(ApiResponse::ok(dto))).into_response()
}

/// Load the median latency (of the answered ones) and the streak of most recent answers from the ledger.
async fn load_entries(state: &AppState, scorecard_id: Uuid) -> Result<(Option<f64>, u32), ()> {
    // Mais recente primeiro: o streak conta as respostas consecutivas do topo.
    let rows: Vec<(String, Option<f64>)> = sqlx::query_as(
        r"SELECT outcome, response_hours
            FROM scorecard_entry
           WHERE scorecard_id = $1
           ORDER BY occurred_at DESC, id DESC",
    )
    .bind(scorecard_id)
    .fetch_all(&state.db)
    .await
    .map_err(|err| tracing::error!(?err, "responsiveness: entries"))?;

    // Outcomes in read order (desc) → streak. Latencies of the answered ones → a pure median.
    let outcomes: Vec<Outcome> = rows
        .iter()
        .filter_map(|(o, _)| o.parse::<Outcome>().ok())
        .collect();
    let hours: Vec<f64> = rows
        .iter()
        .filter(|(o, _)| o == "answered")
        .filter_map(|(_, h)| *h)
        .collect();
    Ok((median_hours(&hours), tier::current_answer_streak(&outcomes)))
}

/// Assemble the peer comparison (same `sphere` + `uf`), excluding this mandate.
async fn load_peer_comparison(
    state: &AppState,
    mandate_id: Uuid,
    sphere: Option<&str>,
    uf: Option<&str>,
    your_rate: Option<u32>,
) -> Result<PeerComparisonDto, ()> {
    let peer_rates = load_peer_rates(state, mandate_id, sphere, uf).await?;
    let scope = uf
        .filter(|u| !u.is_empty())
        .map(str::to_owned)
        .or_else(|| sphere.map(scope_label))
        .unwrap_or_else(|| "Brasil".to_owned());

    let better = your_rate.and_then(|r| tier::better_than_pct(r, &peer_rates));
    Ok(PeerComparisonDto {
        scope,
        peer_count: peer_rates.len() as i64,
        peer_avg_rate: tier::average_rate(&peer_rates),
        better_than_pct: better,
        top_pct: better.map(tier::top_pct),
    })
}

/// The rates (0–100) of comparable peers: mandates of the same level/UF with at least one demand.
async fn load_peer_rates(
    state: &AppState,
    mandate_id: Uuid,
    sphere: Option<&str>,
    uf: Option<&str>,
) -> Result<Vec<u32>, ()> {
    let rows: Vec<(i64, i64)> = sqlx::query_as(
        r"SELECT s.answered, s.ignored
            FROM scorecard s
            JOIN mandate m ON m.id = s.mandate_id
           WHERE m.hidden_at IS NULL
             AND m.id <> $1
             AND ($2::text IS NULL OR m.sphere = $2)
             AND ($3::text IS NULL OR m.uf = $3)
             AND (s.answered + s.ignored) > 0",
    )
    .bind(mandate_id)
    .bind(sphere)
    .bind(uf.filter(|u| !u.is_empty()))
    .fetch_all(&state.db)
    .await
    .map_err(|err| tracing::error!(?err, "responsiveness: peer rates"))?;

    Ok(rows
        .into_iter()
        .filter_map(|(a, i)| tier::response_rate_pct(a, i))
        .collect())
}

fn scope_label(sphere: &str) -> String {
    match sphere {
        "federal" => "nível federal".to_owned(),
        "estadual" => "nível estadual".to_owned(),
        "municipal" => "nível municipal".to_owned(),
        other => other.to_owned(),
    }
}

// ---------------------------------------------------------------------------
// C2 standalone — GET /politicos/responsiveness/peers
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct PeersParams {
    sphere: Option<String>,
    uf: Option<String>,
    house: Option<String>,
    party: Option<String>,
}

#[derive(Debug, Serialize)]
struct PeersAggregateDto {
    /// The queried slice (echoed back for clarity on the front end).
    sphere: Option<String>,
    uf: Option<String>,
    house: Option<String>,
    party: Option<String>,
    /// Mandates with at least one demand in the slice.
    peer_count: i64,
    /// Mean of the individual response rates (0–100).
    avg_response_rate: Option<u32>,
    /// Taxa agregada (soma respondidas / soma total) — o "78% vs 21%" honesto do grupo.
    overall_rate: Option<u32>,
    /// Median response latency of the WHOLE group (hours), via percentile_cont.
    median_response_hours: Option<f64>,
    /// Total answered and ignored in the slice (transparency of the denominator).
    total_answered: i64,
    total_ignored: i64,
}

async fn peers_aggregate(State(state): State<AppState>, Query(p): Query<PeersParams>) -> Response {
    let clean = |v: Option<String>| v.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    let sphere = clean(p.sphere);
    let uf = clean(p.uf).map(|s| s.to_ascii_uppercase());
    let house = clean(p.house);
    let party = clean(p.party);

    // Individual rates + aggregate totals in a single SELECT over the slice.
    let rows: Vec<(i64, i64)> = match sqlx::query_as(
        r"SELECT s.answered, s.ignored
            FROM scorecard s
            JOIN mandate m ON m.id = s.mandate_id
           WHERE m.hidden_at IS NULL
             AND ($1::text IS NULL OR m.sphere = $1)
             AND ($2::text IS NULL OR m.uf = $2)
             AND ($3::text IS NULL OR m.house = $3)
             AND ($4::text IS NULL OR m.party = $4)
             AND (s.answered + s.ignored) > 0",
    )
    .bind(sphere.as_deref())
    .bind(uf.as_deref())
    .bind(house.as_deref())
    .bind(party.as_deref())
    .fetch_all(&state.db)
    .await
    {
        Ok(r) => r,
        Err(err) => {
            tracing::error!(?err, "responsiveness: peers aggregate");
            return server_error();
        }
    };

    let rates: Vec<u32> = rows
        .iter()
        .filter_map(|(a, i)| tier::response_rate_pct(*a, *i))
        .collect();
    let total_answered: i64 = rows.iter().map(|(a, _)| *a).sum();
    let total_ignored: i64 = rows.iter().map(|(_, i)| *i).sum();
    let overall_rate = tier::response_rate_pct(total_answered, total_ignored);

    // Median latency of the whole group — done in Postgres (percentile_cont) so we never pull every
    // ledger row into the process.
    let median: Option<f64> = match sqlx::query_scalar(
        r"SELECT percentile_cont(0.5) WITHIN GROUP (ORDER BY e.response_hours)
            FROM scorecard_entry e
            JOIN scorecard s ON s.id = e.scorecard_id
            JOIN mandate m ON m.id = s.mandate_id
           WHERE e.outcome = 'answered'
             AND e.response_hours IS NOT NULL
             AND m.hidden_at IS NULL
             AND ($1::text IS NULL OR m.sphere = $1)
             AND ($2::text IS NULL OR m.uf = $2)
             AND ($3::text IS NULL OR m.house = $3)
             AND ($4::text IS NULL OR m.party = $4)",
    )
    .bind(sphere.as_deref())
    .bind(uf.as_deref())
    .bind(house.as_deref())
    .bind(party.as_deref())
    .fetch_one(&state.db)
    .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "responsiveness: peers median");
            return server_error();
        }
    };

    let dto = PeersAggregateDto {
        sphere,
        uf,
        house,
        party,
        peer_count: rates.len() as i64,
        avg_response_rate: tier::average_rate(&rates),
        overall_rate,
        median_response_hours: median,
        total_answered,
        total_ignored,
    };
    (StatusCode::OK, Json(ApiResponse::ok(dto))).into_response()
}

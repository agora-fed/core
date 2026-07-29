//! # Responsividade do mandato — a VITRINE positiva do político (Bloco C do plano).
//!
//! Hoje o placar só oferece AMEAÇA (o silêncio vira registro público). Este módulo inverte a
//! economia de reputação: a partir dos MESMOS contadores `answered`/`ignored` e da latência que o
//! `dsoc-scorecard` já projeta (NÃO reinventamos o ledger), derivamos um selo/tier, o "responde em
//! ~N dias", a sequência de respostas (streak) e o comparativo com pares — o motivo POSITIVO pra um
//! vereador/deputado QUERER reivindicar e usar o placar.
//!
//! * `GET /politicos/{mandate_id}/responsiveness` — C1 (selo + streak) + C2 (comparativo com pares
//!   do mesmo nível/UF) num payload só, pronto pra página pública do político.
//! * `GET /politicos/responsiveness/peers?sphere=&uf=&house=&party=` — C2 standalone: agregados de
//!   um recorte (média de resposta, latência mediana, tamanho do grupo).
//!
//! A lógica de decisão (tier/percentil) é PURA e vive testada em `dsoc_scorecard::tier`; aqui só
//! consultamos (runtime sqlx, sem `.sqlx` — mesmo padrão do `politicos_ext.rs`/`og_cards.rs`) e
//! montamos o DTO. LGPD: tudo exposto já é público (o placar É o artefato público de accountability).

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

/// O selo derivado + os números que o sustentam.
#[derive(Debug, Serialize)]
struct TierDto {
    /// Token estável (unrated|building|bronze|silver|gold) — casa com CSS/badge no front.
    key: String,
    /// Rótulo pt-BR ("Ouro", "Prata"…).
    label: String,
    /// Medalha (emoji).
    medal: String,
    /// Explicação de uma linha.
    blurb: String,
}

/// Comparativo com os pares do mesmo nível/UF ("você respondeu 78% · média do RS 21%").
#[derive(Debug, Serialize)]
struct PeerComparisonDto {
    /// Rótulo do recorte comparado (ex.: "RS", ou o nível quando a UF é desconhecida).
    scope: String,
    /// Quantos pares comparáveis (com ao menos uma demanda) existem no recorte.
    peer_count: i64,
    /// Média das taxas de resposta dos pares (0–100).
    peer_avg_rate: Option<u32>,
    /// % de pares que este mandato supera.
    better_than_pct: Option<u32>,
    /// "Top Y%" — complemento do anterior (menor = melhor).
    top_pct: Option<u32>,
}

/// Payload público da responsividade de um mandato.
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
    /// Demandas ignoradas (silêncio público).
    ignored: i64,
    /// Taxa de resposta 0–100 (None quando não há demandas).
    response_rate: Option<u32>,
    /// Latência mediana das respostas, em horas (None quando nada respondido).
    median_response_hours: Option<f64>,
    /// "Responde em ~N dias" (None quando nada respondido).
    responds_in_days: Option<f64>,
    /// Respostas consecutivas mais recentes (medalha de consistência).
    answer_streak: u32,
    /// O selo/tier.
    tier: TierDto,
    /// Comparativo com pares.
    peer: PeerComparisonDto,
}

/// Linha do mandato + contadores do placar (LEFT JOIN: mandato sem placar ainda conta 0/0).
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

    // Latência mediana + streak vêm do ledger (scorecard_entry), quando há placar.
    let (median, streak) = match scorecard_id {
        Some(sid) => match load_entries(&state, sid).await {
            Ok((med, strk)) => (med, strk),
            Err(()) => return server_error(),
        },
        None => (None, 0),
    };

    let response_rate = tier::response_rate_pct(answered, ignored);
    let tier_val = tier::responsiveness_tier(answered, ignored, median);

    // Comparativo com pares: mesmo nível (sphere) + mesma UF, excluindo o próprio.
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

/// Carrega latência mediana (das respondidas) e o streak de respostas mais recentes do ledger.
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

    // Outcomes na ordem lida (desc) → streak. Latências das respondidas → mediana pura.
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

/// Monta o comparativo com pares (mesmo `sphere` + `uf`), excluindo o próprio mandato.
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

/// As taxas (0–100) dos pares comparáveis: mandatos do mesmo nível/UF com ao menos uma demanda.
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
    /// Recorte consultado (ecoado de volta pra clareza no front).
    sphere: Option<String>,
    uf: Option<String>,
    house: Option<String>,
    party: Option<String>,
    /// Mandatos com ao menos uma demanda no recorte.
    peer_count: i64,
    /// Média das taxas de resposta individuais (0–100).
    avg_response_rate: Option<u32>,
    /// Taxa agregada (soma respondidas / soma total) — o "78% vs 21%" honesto do grupo.
    overall_rate: Option<u32>,
    /// Latência mediana das respostas de TODO o grupo (horas), via percentile_cont.
    median_response_hours: Option<f64>,
    /// Total de respondidas e ignoradas no recorte (transparência do denominador).
    total_answered: i64,
    total_ignored: i64,
}

async fn peers_aggregate(State(state): State<AppState>, Query(p): Query<PeersParams>) -> Response {
    let clean = |v: Option<String>| v.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    let sphere = clean(p.sphere);
    let uf = clean(p.uf).map(|s| s.to_ascii_uppercase());
    let house = clean(p.house);
    let party = clean(p.party);

    // Taxas individuais + totais agregados num só SELECT sobre o recorte.
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

    // Latência mediana do grupo inteiro — feita no Postgres (percentile_cont) pra não puxar todas
    // as linhas do ledger pro processo.
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

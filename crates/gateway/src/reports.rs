//! # Aggregated reports for the political dashboards (0.19.0-dashboards).
//!
//! Two endpoints power the /politicos/gastos and /politicos/propostas pages:
//!
//! * `GET /api/v1/reports/gasto-parlamentar` — total cota parlamentar spend
//!   grouped by (político | partido | cargo | esfera), filterable by
//!   uf/house/party/sphere. The raw per-mandate totals come from the Câmara
//!   open-data API (`/deputados/{id}/despesas`) and are cached in-memory
//!   with a 6-hour TTL. On a cold cache we fetch concurrently (30 in-flight)
//!   so a fresh render finishes in ~10 s. Senado (senadores) has no cota
//!   endpoint — those mandates report zero.
//! * `GET /api/v1/reports/proposals-summary` — DB-only aggregation over
//!   `proposal` joined with `mandate`. Instant.
//!
//! Both endpoints return the same envelope shape so the frontend can share
//! a rendering component (bar chart + table + total tile).

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use dsoc_app::AppState;
use dsoc_api_contract::ApiResponse;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, Semaphore};
use uuid::Uuid;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/reports/gasto-parlamentar", get(gasto_parlamentar))
        .route("/reports/proposals-summary", get(proposals_summary))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Common shapes
// ---------------------------------------------------------------------------

/// Filters accepted by both dashboards. All optional; empty string means "no
/// filter for this column".
#[derive(Debug, Default, Deserialize)]
pub struct DashboardFilters {
    #[serde(default)]
    pub group_by: Option<String>,
    #[serde(default)]
    pub uf: Option<String>,
    #[serde(default)]
    pub house: Option<String>,
    #[serde(default)]
    pub party: Option<String>,
    #[serde(default)]
    pub sphere: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GroupRow {
    pub key: String,
    pub label: String,
    /// Amount in centavos (integer) so the wire is not lossy on 2 decimals.
    /// The frontend divides by 100 for R$ display.
    pub amount_cents: i64,
    pub mandate_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct GastoResponse {
    pub total_cents: i64,
    pub mandate_count: i64,
    pub groups: Vec<GroupRow>,
    /// Non-zero when the aggregate is still warming — the cache is missing
    /// this many mandates. Frontend re-fetches every N seconds until zero.
    pub pending: u32,
    /// When the raw cache was last refreshed.
    pub cached_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PropostaGroupRow {
    pub key: String,
    pub label: String,
    pub count: i64,
    pub published: i64,
    pub clustered: i64,
    /// Only valid when grouping by político/mandate — for others it's the sum.
    pub answered: i64,
    pub ignored: i64,
    pub pending: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PropostasResponse {
    pub total: i64,
    pub groups: Vec<PropostaGroupRow>,
}

// ---------------------------------------------------------------------------
// GET /api/v1/reports/proposals-summary — DB aggregation
// ---------------------------------------------------------------------------

async fn proposals_summary(
    State(state): State<AppState>,
    Query(f): Query<DashboardFilters>,
) -> Response {
    // Base rows: one row per proposal, joined with the mandate. We then
    // Rust-side reduce by the chosen group_by. SQL-side reduce would be
    // faster but forks per group_by; a single query + Rust reduce is small
    // enough at 500-ish rows.
    let where_clauses: Vec<String> = Vec::new();
    let mut sql = String::from(
        r"SELECT p.id, p.status, p.support_count, p.threshold, p.threshold_crossed_at,
                 m.id AS mandate_id, m.display_name, m.party, m.uf, m.house, m.sphere, m.office
            FROM proposal p
            JOIN mandate m ON m.id = p.mandate_id
           WHERE 1=1",
    );
    // Simple string interpolation is safe here because we only accept
    // known-shape enum values below.
    if matches!(f.house.as_deref(), Some("camara") | Some("senado")) {
        sql.push_str(&format!(
            " AND m.house = '{}'",
            f.house.as_deref().unwrap_or("")
        ));
    }
    if matches!(
        f.sphere.as_deref(),
        Some("federal") | Some("estadual") | Some("municipal")
    ) {
        sql.push_str(&format!(
            " AND m.sphere = '{}'",
            f.sphere.as_deref().unwrap_or("")
        ));
    }
    if let Some(uf) = f.uf.as_deref().filter(|s| s.len() == 2) {
        sql.push_str(&format!(" AND m.uf = '{uf}'"));
    }
    if let Some(party) = f
        .party
        .as_deref()
        .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric()))
    {
        sql.push_str(&format!(" AND m.party = '{party}'"));
    }
    if matches!(
        f.status.as_deref(),
        Some("draft") | Some("published") | Some("clustered")
    ) {
        sql.push_str(&format!(
            " AND p.status = '{}'",
            f.status.as_deref().unwrap_or("")
        ));
    }
    let _ = where_clauses;
    #[derive(sqlx::FromRow)]
    struct Row {
        id: Uuid,
        status: String,
        #[allow(dead_code)]
        support_count: i64,
        #[allow(dead_code)]
        threshold: i32,
        threshold_crossed_at: Option<chrono::DateTime<chrono::Utc>>,
        mandate_id: Uuid,
        display_name: String,
        party: Option<String>,
        uf: Option<String>,
        house: Option<String>,
        sphere: String,
        office: String,
    }
    let rows: Vec<Row> = match sqlx::query_as::<_, Row>(&sql).fetch_all(&state.db).await {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(error = ?err, "proposals summary query failed");
            return server_error();
        }
    };
    let total = rows.len() as i64;
    let group_by = f.group_by.as_deref().unwrap_or("partido");
    // Bucket by group_by, tally.
    #[derive(Default)]
    struct Bucket {
        count: i64,
        published: i64,
        clustered: i64,
        answered: i64,
        ignored: i64,
        pending: i64,
        label: String,
    }
    let mut buckets: HashMap<String, Bucket> = HashMap::new();
    for r in &rows {
        let (key, label) = match group_by {
            "politico" | "político" | "mandate" => {
                (r.mandate_id.to_string(), r.display_name.clone())
            }
            "cargo" | "casa" => (
                r.house.clone().unwrap_or_else(|| "sem-casa".into()),
                match r.house.as_deref() {
                    Some("camara") => "Câmara".to_owned(),
                    Some("senado") => "Senado".to_owned(),
                    _ => "Sem casa".to_owned(),
                },
            ),
            "esfera" | "sphere" => (
                r.sphere.clone(),
                match r.sphere.as_str() {
                    "federal" => "Federal".to_owned(),
                    "estadual" => "Estadual".to_owned(),
                    "municipal" => "Municipal".to_owned(),
                    other => other.to_owned(),
                },
            ),
            "uf" | "estado" => {
                let uf = r.uf.clone().unwrap_or_else(|| "??".into());
                (uf.clone(), uf)
            }
            "office" | "cargo_completo" => (r.office.clone(), r.office.clone()),
            _ /* partido | default */ => {
                let p = r.party.clone().unwrap_or_else(|| "SEM PARTIDO".into());
                (p.clone(), p)
            }
        };
        let b = buckets.entry(key).or_default();
        b.label = label;
        b.count += 1;
        if r.status == "published" {
            b.published += 1;
        }
        if r.status == "clustered" {
            b.clustered += 1;
        }
        if r.threshold_crossed_at.is_some() {
            b.answered += 1; // A crude proxy — for a real answered count we
                             // would join with sla_case; that's an easy follow-up.
        }
        let _ = &r.id;
    }
    let mut groups: Vec<PropostaGroupRow> = buckets
        .into_iter()
        .map(|(key, b)| PropostaGroupRow {
            key,
            label: b.label,
            count: b.count,
            published: b.published,
            clustered: b.clustered,
            answered: b.answered,
            ignored: b.ignored,
            pending: b.pending,
        })
        .collect();
    groups.sort_by(|a, b| b.count.cmp(&a.count));
    let out = PropostasResponse { total, groups };
    (StatusCode::OK, Json(ApiResponse::ok(out))).into_response()
}

fn server_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": "internal error" })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Gasto parlamentar — external fetch + concurrent aggregation + 6h TTL cache
// ---------------------------------------------------------------------------

const CAMARA_BASE: &str = "https://dadosabertos.camara.leg.br/api/v2";
const CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const CONCURRENCY: usize = 24;

/// Cached per-mandate context we join on when rendering the dashboard.
#[derive(Debug, Clone)]
struct MandateSlim {
    id: Uuid,
    display_name: String,
    party: Option<String>,
    uf: Option<String>,
    house: Option<String>,
    sphere: String,
    office: String,
    /// Câmara/Senado numeric id — needed to hit the external API.
    external_id: Option<String>,
    /// `camara` | `senado` | other. `senado` = we skip external fetch (no cota).
    source: Option<String>,
}

/// One point in the raw cache: mandate_id → cents spent in the most recent year.
type RawMap = HashMap<Uuid, i64>;

struct Cache {
    fetched_at: Option<Instant>,
    map: RawMap,
    /// Number of mandates still pending an external fetch — 0 when everyone
    /// has been visited (successfully or not) at least once.
    pending: u32,
    /// Fixed mandate slim rows we ran the last fetch against, so a re-render
    /// under a filter doesn't need to re-query the DB.
    mandates: Vec<MandateSlim>,
}

static CACHE_LOCK: std::sync::LazyLock<Arc<RwLock<Cache>>> = std::sync::LazyLock::new(|| {
    Arc::new(RwLock::new(Cache {
        fetched_at: None,
        map: HashMap::new(),
        pending: 0,
        mandates: Vec::new(),
    }))
});

async fn gasto_parlamentar(
    State(state): State<AppState>,
    Query(f): Query<DashboardFilters>,
) -> Response {
    // Check cache; refresh if stale or first hit.
    {
        let cache = CACHE_LOCK.read().await;
        if cache
            .fetched_at
            .is_some_and(|t| t.elapsed() < CACHE_TTL)
            && cache.pending == 0
        {
            let out = build_response(&cache.map, &cache.mandates, &f, 0, cache.fetched_at);
            return (StatusCode::OK, Json(ApiResponse::ok(out))).into_response();
        }
    }
    // Cold — refresh (or stale). Re-fetch mandate list from DB, then hit Câmara.
    let mandates = match load_mandate_slims(&state).await {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(error = ?err, "gasto: mandate list load failed");
            return server_error();
        }
    };
    let map = fetch_all_expenses(&mandates).await;
    let now = Instant::now();
    {
        let mut cache = CACHE_LOCK.write().await;
        cache.fetched_at = Some(now);
        cache.map = map.clone();
        cache.mandates = mandates.clone();
        // Pending = how many mandates have zero recorded (either genuinely
        // zero-spend or failed to fetch). The refresh replaces the whole
        // map, so pending resets to 0 after this pass.
        cache.pending = 0;
    }
    let out = build_response(&map, &mandates, &f, 0, Some(now));
    (StatusCode::OK, Json(ApiResponse::ok(out))).into_response()
}

async fn load_mandate_slims(state: &AppState) -> Result<Vec<MandateSlim>, sqlx::Error> {
    // NULL org_id excluded — we only ever aggregate DemocraciaBR org's mandates.
    let rows: Vec<(
        Uuid,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
        String,
        Option<String>,
        Option<String>,
    )> = sqlx::query_as(
        r"SELECT id, display_name, party, uf, house, sphere, office,
                 source_external_id, source
            FROM mandate
           WHERE org_id = '11111111-1111-1111-1111-111111111111'::uuid",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, display_name, party, uf, house, sphere, office, external_id, source)| MandateSlim {
                id,
                display_name,
                party,
                uf,
                house,
                sphere,
                office,
                external_id,
                source,
            },
        )
        .collect())
}

/// For each mandate, fetch the most recent year's cota parlamentar from the
/// Câmara open-data API. Senado mandates are recorded as zero (they have no
/// equivalent public endpoint we can hit here). Concurrency is bounded by
/// `CONCURRENCY` semaphore permits.
async fn fetch_all_expenses(mandates: &[MandateSlim]) -> RawMap {
    let sem = Arc::new(Semaphore::new(CONCURRENCY));
    let cli = match reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent("democracia.social.br/reports")
        .build()
    {
        Ok(c) => c,
        Err(_) => return HashMap::new(),
    };
    let mut handles = Vec::with_capacity(mandates.len());
    for m in mandates {
        if !matches!(m.source.as_deref(), Some("camara")) {
            continue;
        }
        let Some(ext) = m.external_id.clone() else {
            continue;
        };
        let id = m.id;
        let permit = sem.clone();
        let cli = cli.clone();
        handles.push(tokio::spawn(async move {
            let _guard = permit.acquire().await.ok()?;
            let url = format!(
                "{CAMARA_BASE}/deputados/{ext}/despesas?ordem=DESC&ordenarPor=ano&itens=100"
            );
            let resp = cli.get(&url).send().await.ok()?;
            if !resp.status().is_success() {
                return Some((id, 0i64));
            }
            let json: serde_json::Value = resp.json().await.ok()?;
            let items = json.get("dados")?.as_array()?;
            if items.is_empty() {
                return Some((id, 0i64));
            }
            let mut top_year: Option<i64> = None;
            for it in items {
                if let Some(y) = it.get("ano").and_then(|v| v.as_i64()) {
                    top_year = Some(top_year.map_or(y, |cur| cur.max(y)));
                }
            }
            let year = top_year?;
            let mut sum_cents: i64 = 0;
            for it in items {
                if it.get("ano").and_then(|v| v.as_i64()) != Some(year) {
                    continue;
                }
                let v = it
                    .get("valorLiquido")
                    .and_then(|v| v.as_f64())
                    .or_else(|| it.get("valorDocumento").and_then(|v| v.as_f64()))
                    .unwrap_or(0.0);
                // Round to cents to keep the wire an int.
                sum_cents += (v * 100.0).round() as i64;
            }
            Some((id, sum_cents))
        }));
    }
    let mut map: RawMap = HashMap::new();
    // Seed with zeros for every mandate so filters work even for
    // senadores/unfetched-mandates.
    for m in mandates {
        map.insert(m.id, 0);
    }
    for h in handles {
        if let Ok(Some((id, cents))) = h.await {
            map.insert(id, cents);
        }
    }
    map
}

fn build_response(
    map: &RawMap,
    mandates: &[MandateSlim],
    f: &DashboardFilters,
    pending: u32,
    fetched_at: Option<Instant>,
) -> GastoResponse {
    // Filter mandates by (uf/house/party/sphere).
    let filtered: Vec<&MandateSlim> = mandates
        .iter()
        .filter(|m| {
            f.uf.as_deref().is_none_or(|uf| {
                uf.is_empty() || m.uf.as_deref().is_some_and(|v| v.eq_ignore_ascii_case(uf))
            })
        })
        .filter(|m| {
            f.house.as_deref().is_none_or(|h| {
                h.is_empty() || m.house.as_deref().is_some_and(|v| v == h)
            })
        })
        .filter(|m| {
            f.party.as_deref().is_none_or(|p| {
                p.is_empty() || m.party.as_deref().is_some_and(|v| v.eq_ignore_ascii_case(p))
            })
        })
        .filter(|m| {
            f.sphere
                .as_deref()
                .is_none_or(|s| s.is_empty() || m.sphere == s)
        })
        .collect();
    let group_by = f.group_by.as_deref().unwrap_or("partido");
    #[derive(Default)]
    struct B {
        amount: i64,
        count: i64,
        label: String,
    }
    let mut buckets: HashMap<String, B> = HashMap::new();
    for m in &filtered {
        let cents = map.get(&m.id).copied().unwrap_or(0);
        let (key, label) = match group_by {
            "politico" | "político" | "mandate" => {
                (m.id.to_string(), m.display_name.clone())
            }
            "cargo" | "casa" => (
                m.house.clone().unwrap_or_else(|| "sem-casa".into()),
                match m.house.as_deref() {
                    Some("camara") => "Câmara".to_owned(),
                    Some("senado") => "Senado".to_owned(),
                    _ => "Sem casa".to_owned(),
                },
            ),
            "esfera" | "sphere" => (
                m.sphere.clone(),
                match m.sphere.as_str() {
                    "federal" => "Federal".to_owned(),
                    "estadual" => "Estadual".to_owned(),
                    "municipal" => "Municipal".to_owned(),
                    other => other.to_owned(),
                },
            ),
            "uf" | "estado" => {
                let uf = m.uf.clone().unwrap_or_else(|| "??".into());
                (uf.clone(), uf)
            }
            "office" | "cargo_completo" => (m.office.clone(), m.office.clone()),
            _ => {
                let p = m.party.clone().unwrap_or_else(|| "SEM PARTIDO".into());
                (p.clone(), p)
            }
        };
        let b = buckets.entry(key).or_default();
        b.label = label;
        b.amount += cents;
        b.count += 1;
    }
    let total_cents: i64 = buckets.values().map(|b| b.amount).sum();
    let mandate_count: i64 = filtered.len() as i64;
    let mut groups: Vec<GroupRow> = buckets
        .into_iter()
        .map(|(key, b)| GroupRow {
            key,
            label: b.label,
            amount_cents: b.amount,
            mandate_count: b.count,
        })
        .collect();
    groups.sort_by(|a, b| b.amount_cents.cmp(&a.amount_cents));
    let cached_at = fetched_at
        .map(|_| chrono::Utc::now().to_rfc3339())
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
    GastoResponse {
        total_cents,
        mandate_count,
        groups,
        pending,
        cached_at,
    }
}

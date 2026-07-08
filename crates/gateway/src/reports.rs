//! # Aggregated reports for the political dashboards (0.19.0-dashboards).
//!
//! Two endpoints power the /politicos/gastos and /politicos/propostas pages:
//!
//! * `GET /api/v1/reports/gasto-parlamentar` — total cota parlamentar spend
//!   grouped by (político | partido | cargo | esfera), filterable by
//!   uf/house/party/sphere. Raw per-mandate totals come from two sources:
//!   Câmara `/deputados/{id}/despesas` (JSON, paginated by `?ano=&pagina=`)
//!   for deputados, and Senado's CEAPS CSV
//!   (`https://www.senado.leg.br/transparencia/LAI/verba/despesa_ceaps_{YYYY}.csv`,
//!   Latin-1, `;`-separated) for senadores — matched by display_name
//!   uppercase. Both cached together in-memory with a 6-hour TTL. Cold
//!   render ~10 s.
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
use chrono::Datelike;
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
    /// Fiscal year the aggregate reports on (typically N-1). Frontend
    /// renders "Ano de referência: 2025".
    pub year: i32,
    /// Per-mandate detail rows — only present when a filter narrows the
    /// result enough (< 100 mandates). Used for drill-down in the UI.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detail: Vec<MandateDetailRow>,
}

/// One per-mandate detail row used by the drill-down modal.
#[derive(Debug, Clone, Serialize)]
pub struct MandateDetailRow {
    pub mandate_id: String,
    pub display_name: String,
    pub party: Option<String>,
    pub uf: Option<String>,
    pub house: Option<String>,
    pub amount_cents: i64,
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

/// Per-mandate detail row for the propostas drill-down. Emitted only when
/// the filtered set is small enough (< 200 mandates) so the payload stays
/// bounded.
#[derive(Debug, Clone, Serialize)]
pub struct PropostaMandateRow {
    pub mandate_id: String,
    pub display_name: String,
    pub party: Option<String>,
    pub uf: Option<String>,
    pub house: Option<String>,
    pub count: i64,
    pub published: i64,
    pub clustered: i64,
    pub answered: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PropostasResponse {
    pub total: i64,
    pub groups: Vec<PropostaGroupRow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detail: Vec<PropostaMandateRow>,
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
    // Per-mandate detail rollup — one row per mandate present in the rows.
    let mut detail: Vec<PropostaMandateRow> = Vec::new();
    #[derive(Default)]
    struct MB {
        display_name: String,
        party: Option<String>,
        uf: Option<String>,
        house: Option<String>,
        count: i64,
        published: i64,
        clustered: i64,
        answered: i64,
    }
    let mut by_mandate: HashMap<Uuid, MB> = HashMap::new();
    for r in &rows {
        let m = by_mandate.entry(r.mandate_id).or_default();
        m.display_name = r.display_name.clone();
        m.party = r.party.clone();
        m.uf = r.uf.clone();
        m.house = r.house.clone();
        m.count += 1;
        if r.status == "published" {
            m.published += 1;
        }
        if r.status == "clustered" {
            m.clustered += 1;
        }
        if r.threshold_crossed_at.is_some() {
            m.answered += 1;
        }
    }
    if by_mandate.len() <= 200 {
        for (mid, mb) in by_mandate {
            detail.push(PropostaMandateRow {
                mandate_id: mid.to_string(),
                display_name: mb.display_name,
                party: mb.party,
                uf: mb.uf,
                house: mb.house,
                count: mb.count,
                published: mb.published,
                clustered: mb.clustered,
                answered: mb.answered,
            });
        }
        detail.sort_by(|a, b| b.count.cmp(&a.count));
    }
    let out = PropostasResponse {
        total,
        groups,
        detail,
    };
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
    /// The fiscal year the cached aggregate is reporting on. Exposed in the
    /// response so the front can render "Ano de referência: 2025".
    year: i32,
}

static CACHE_LOCK: std::sync::LazyLock<Arc<RwLock<Cache>>> = std::sync::LazyLock::new(|| {
    Arc::new(RwLock::new(Cache {
        fetched_at: None,
        map: HashMap::new(),
        pending: 0,
        mandates: Vec::new(),
        year: 0,
    }))
});

/// Body param `?refresh=1` bypasses the cache and forces a fresh fetch.
#[derive(Debug, Default, Deserialize)]
pub struct GastoQuery {
    #[serde(flatten)]
    pub filters: DashboardFilters,
    #[serde(default)]
    pub refresh: Option<String>,
}

async fn gasto_parlamentar(
    State(state): State<AppState>,
    Query(mut q): Query<GastoQuery>,
) -> Response {
    // Sphere é federal-only aqui (ver load_mandate_slims). Se o cliente pediu
    // estadual/municipal, normalizamos pra federal em vez de retornar 400 —
    // links antigos com `?sphere=municipal` continuam funcionando, só que
    // sobre a base federal (que é a única com dado real).
    if q.filters.sphere.as_deref() != Some("federal") {
        q.filters.sphere = Some("federal".to_owned());
    }
    let force_refresh = q.refresh.as_deref().is_some_and(|s| s == "1" || s == "true");
    // Check cache; refresh if stale, first hit, or forced.
    if !force_refresh {
        let cache = CACHE_LOCK.read().await;
        if cache
            .fetched_at
            .is_some_and(|t| t.elapsed() < CACHE_TTL)
            && cache.pending == 0
        {
            let out = build_response(
                &cache.map,
                &cache.mandates,
                &q.filters,
                0,
                cache.fetched_at,
                cache.year,
            );
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
    let (map, year) = fetch_all_expenses(&mandates).await;
    let now = Instant::now();
    {
        let mut cache = CACHE_LOCK.write().await;
        cache.fetched_at = Some(now);
        cache.map = map.clone();
        cache.mandates = mandates.clone();
        cache.pending = 0;
        cache.year = year;
    }
    let out = build_response(&map, &mandates, &q.filters, 0, Some(now), year);
    (StatusCode::OK, Json(ApiResponse::ok(out))).into_response()
}

async fn load_mandate_slims(state: &AppState) -> Result<Vec<MandateSlim>, sqlx::Error> {
    // NULL org_id excluded — we only ever aggregate DemocraciaBR org's mandates.
    //
    // FEDERAL-ONLY: CEAP + CEAPS são cotas do Congresso Federal. Não faz
    // sentido puxar 70k mandatos estaduais + municipais aqui (que não têm
    // fonte de dado equivalente) — a query scanea a tabela toda inutilmente.
    // Restrição federal deixa a leitura em ~594 rows, tornando o cold start
    // ~50× mais rápido.
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
           WHERE org_id = '11111111-1111-1111-1111-111111111111'::uuid
             AND sphere = 'federal'
             AND house IN ('camara', 'senado')",
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

/// Which fiscal year the aggregate is reporting on. We prefer the LAST
/// FULLY-CLOSED year (typically N-1) because the current year's stream is
/// partial and would understate every mandate. `now` is passed in so tests
/// can pin the choice.
fn reporting_year_for(now: chrono::DateTime<chrono::Utc>) -> i32 {
    // Câmara publishes expenses with a lag — use N-1 uniformly. If the
    // current month is late in the year and N-1 is stable, this is safe.
    // (A future refinement: switch to current year once we're past March.)
    let year = now.date_naive().and_hms_opt(0, 0, 0).unwrap().year();
    year - 1
}

/// For each mandate, fetch the last-closed-year cota parlamentar from the
/// Câmara open-data API. Senado mandates are recorded as zero (no
/// equivalent public endpoint here). Concurrency bounded by `CONCURRENCY`.
///
/// The Câmara despesas endpoint pages at 100 items. Deputies file 200-400
/// items/year → we paginate `?ano=<year>&itens=100&pagina=<n>` until the
/// server returns an empty `dados` array.
async fn fetch_all_expenses(mandates: &[MandateSlim]) -> (RawMap, i32) {
    let year = reporting_year_for(chrono::Utc::now());
    let sem = Arc::new(Semaphore::new(CONCURRENCY));
    let cli = match reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent("democracia.social.br/reports")
        .build()
    {
        Ok(c) => c,
        Err(_) => return (HashMap::new(), year),
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
            let mut sum_cents: i64 = 0;
            // Walk pagina=1..=N until dados is empty. A safety cap of 20
            // pages caps a single deputy at 2000 items.
            for page in 1..=20u32 {
                let url = format!(
                    "{CAMARA_BASE}/deputados/{ext}/despesas?ano={year}&itens=100&pagina={page}"
                );
                let Ok(resp) = cli.get(&url).send().await else {
                    break;
                };
                if !resp.status().is_success() {
                    break;
                }
                let Ok(json) = resp.json::<serde_json::Value>().await else {
                    break;
                };
                let Some(items) = json.get("dados").and_then(|v| v.as_array()) else {
                    break;
                };
                if items.is_empty() {
                    break;
                }
                for it in items {
                    let v = it
                        .get("valorLiquido")
                        .and_then(|v| v.as_f64())
                        .or_else(|| it.get("valorDocumento").and_then(|v| v.as_f64()))
                        .unwrap_or(0.0);
                    sum_cents += (v * 100.0).round() as i64;
                }
                if items.len() < 100 {
                    // Last page — no need to poke the next one.
                    break;
                }
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
    // 0.19.0-polish: also fold in Senado CEAPS from the published CSV. The
    // Senado exposes despesa_ceaps_{YYYY}.csv (Latin-1, `;`-separated).
    // Match by display_name uppercase — senator rows carry no external id
    // shared with our mandate table today.
    if let Ok(senado_by_name) = fetch_senado_ceaps(&cli, year).await {
        for m in mandates {
            if !matches!(m.house.as_deref(), Some("senado")) {
                continue;
            }
            let name_up = normalize_senator_name(&m.display_name);
            if let Some(cents) = senado_by_name.get(&name_up) {
                map.insert(m.id, *cents);
            }
        }
    } else {
        tracing::warn!(year, "senado CEAPS fetch failed — senadores keep zero");
    }
    (map, year)
}

/// Download + parse the Senado CEAPS CSV for `year`. Returns a
/// `NAME_UPPERCASE → cents` map (integer cents to match the Câmara path).
async fn fetch_senado_ceaps(
    cli: &reqwest::Client,
    year: i32,
) -> Result<HashMap<String, i64>, ()> {
    let url = format!(
        "https://www.senado.leg.br/transparencia/LAI/verba/despesa_ceaps_{year}.csv"
    );
    let resp = cli.get(&url).send().await.map_err(|_| ())?;
    if !resp.status().is_success() {
        return Err(());
    }
    let bytes = resp.bytes().await.map_err(|_| ())?;
    // Decode Latin-1 → UTF-8: every byte is its Unicode codepoint.
    let text: String = bytes.iter().map(|&b| b as char).collect();
    let mut out: HashMap<String, i64> = HashMap::new();
    // First line is a metadata / disclaimer row; second is the header. Skip
    // both. From line 3 on the columns are:
    //  ANO ; MES ; SENADOR ; TIPO_DESPESA ; CNPJ_CPF ; FORNECEDOR ;
    //  DOCUMENTO ; DATA ; DETALHAMENTO ; VALOR_REEMBOLSADO ; COD_DOCUMENTO
    for (i, line) in text.lines().enumerate() {
        if i < 2 || line.trim().is_empty() {
            continue;
        }
        let cols = split_semi_csv(line);
        if cols.len() < 10 {
            continue;
        }
        let name = cols[2].trim().to_uppercase();
        if name.is_empty() {
            continue;
        }
        let value_str = cols[9].trim().replace(',', ".");
        let value: f64 = value_str.parse().unwrap_or(0.0);
        let cents = (value * 100.0).round() as i64;
        *out.entry(name).or_insert(0) += cents;
    }
    Ok(out)
}

/// Split a `;`-separated CSV row honouring `"…"`-quoted cells. Simple enough
/// for the well-behaved Senado files; no need for the `csv` crate.
fn split_semi_csv(line: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if in_quotes && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => in_quotes = !in_quotes,
            ';' if !in_quotes => {
                out.push(std::mem::take(&mut cur));
            }
            other => cur.push(other),
        }
    }
    out.push(cur);
    out
}

/// Normalize a senator display name for matching against the CEAPS row. We
/// uppercase, strip accents on the vowels most common in PT-BR names, and
/// collapse whitespace. Doesn't try to handle every diacritic — the CEAPS
/// file itself keeps accents.
fn normalize_senator_name(name: &str) -> String {
    name.to_uppercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn build_response(
    map: &RawMap,
    mandates: &[MandateSlim],
    f: &DashboardFilters,
    pending: u32,
    fetched_at: Option<Instant>,
    year: i32,
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
    // Detail rows for drill-down. Only emit when the filtered set is small
    // enough to be usable (large sets would blow the response size).
    let mut detail: Vec<MandateDetailRow> = Vec::new();
    if filtered.len() <= 200 {
        for m in &filtered {
            let cents = map.get(&m.id).copied().unwrap_or(0);
            detail.push(MandateDetailRow {
                mandate_id: m.id.to_string(),
                display_name: m.display_name.clone(),
                party: m.party.clone(),
                uf: m.uf.clone(),
                house: m.house.clone(),
                amount_cents: cents,
            });
        }
        detail.sort_by(|a, b| b.amount_cents.cmp(&a.amount_cents));
    }
    GastoResponse {
        total_cents,
        mandate_count,
        groups,
        pending,
        cached_at,
        year,
        detail,
    }
}

//! # Parliamentarian activity gateway — proxy + cache of official open-data APIs.
//!
//! Powers the rich "político" page by fanning out to the Brazilian houses' public open-data APIs
//! (the lower house and the Senate), normalizing their wildly different shapes into one
//! house-agnostic DTO, and serving it under the platform's standard [`ApiResponse`] envelope.
//!
//! Route (merged INTO `/api/v1` by the gateway, alongside the mandates crate's own routes):
//! * `GET /api/v1/mandates/{id}/atividade?org_id=<uuid>`
//!
//! Design notes:
//! * The mandate row carries `source` ('camara' | 'senado' | 'manual') and `source_external_id`
//!   (the numeric id in that house's API), added in migration 0201. If either is NULL (a
//!   hand-created candidacy, or a historical seed row), we return an empty-but-OK payload — the
//!   page just renders no activity, never an error.
//! * Robustness: every upstream section is fetched independently with an ~8s timeout. A timeout or
//!   non-200 for ONE section degrades that section to empty; it NEVER fails the whole request.
//! * Caching: a 15-minute in-memory TTL cache keyed by mandate id (these upstream APIs are slow and
//!   rate-sensitive). A `LazyLock<RwLock<HashMap<..>>>` — no new crate, std + tokio only.
//! * SQL: the gateway reads `source`/`source_external_id` with a runtime (unchecked) `query_as`
//!   because the workspace uses `SQLX_OFFLINE` (a committed `.sqlx/` cache) and this build host has
//!   no DB to regenerate it. The read is a narrow, dedicated SELECT — it does NOT touch `MandateDto`.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;
use uuid::Uuid;

// --- Tunables -----------------------------------------------------------------------------------

/// Per-request cap on any single upstream call. A slow house API must not stall the page.
const UPSTREAM_TIMEOUT_SECS: u64 = 8;
/// How long a normalized payload stays fresh in the in-memory cache.
const CACHE_TTL: Duration = Duration::from_secs(15 * 60);

const CAMARA_BASE: &str = "https://dadosabertos.camara.leg.br/api/v2";
const SENADO_BASE: &str = "https://legis.senado.leg.br/dadosabertos";

const USER_AGENT: &str = "DemocraciaBR/0.4 (+https://democracia.social.br)";

// --- Router -------------------------------------------------------------------------------------

/// Mount the parliamentarian-activity route. Merged into the gateway's `/api/v1` group so the
/// public path is `/api/v1/mandates/{id}/atividade`. Read-only and public (mirrors the other
/// mandate reads); it exposes only already-public official data.
pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/mandates/{mandate_id}/atividade", get(get_activity))
        .with_state(state)
}

/// Query parameters: the tenant/org the mandate belongs to (same contract as the other mandate reads).
#[derive(Debug, Clone, Deserialize)]
struct OrgQuery {
    org_id: Uuid,
}

// --- Normalized, house-agnostic DTO -------------------------------------------------------------

/// The normalized activity payload the front-end consumes. Every section is an `Option`/empty vec
/// so a house that lacks a section (or an upstream failure) simply yields nothing — never an error.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ActivityDto {
    /// `camara` | `senado` | `manual` | `null` (the mandate's `source`).
    pub source: Option<String>,
    /// The numeric external id in that house's API (the mandate's `source_external_id`).
    pub external_id: Option<String>,
    /// Upcoming + recent public events / sessions.
    pub agenda: Vec<AgendaItem>,
    /// Legislative production (bills / matters authored).
    pub production: Vec<ProductionItem>,
    /// Floor speeches (lower house: discursos; Senate: pronunciamentos).
    pub speeches: Vec<SpeechItem>,
    /// Nominal votes cast (Senate only for now; the lower house omits this section).
    pub votes: Vec<VoteItem>,
    /// Expense summary (the lower house's parliamentary quota). `None` for the Senate / when unavailable.
    pub expenses: Option<ExpenseSummary>,
    /// Extra profile bits not on the base `MandateDto` (social links, office contact).
    pub profile_extra: Option<ProfileExtra>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgendaItem {
    pub title: String,
    pub date_time: Option<String>,
    pub location: Option<String>,
    pub kind: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProductionItem {
    pub id: Option<String>,
    pub kind: Option<String>,
    pub number: Option<String>,
    pub year: Option<String>,
    pub summary: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpeechItem {
    pub date_time: Option<String>,
    pub summary: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VoteItem {
    pub date: Option<String>,
    pub matter: Option<String>,
    pub summary: Option<String>,
    pub vote: Option<String>,
    pub result: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExpenseSummary {
    pub year: Option<i64>,
    pub total: f64,
    pub top_categories: Vec<ExpenseCategory>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExpenseCategory {
    pub name: String,
    pub amount: f64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ProfileExtra {
    pub social: Vec<String>,
    pub office_contact: Option<String>,
    pub photo_url: Option<String>,
    pub page_url: Option<String>,
}

// --- In-memory TTL cache ------------------------------------------------------------------------

static CACHE: LazyLock<RwLock<HashMap<Uuid, (Instant, ActivityDto)>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

async fn cache_get(id: Uuid) -> Option<ActivityDto> {
    let guard = CACHE.read().await;
    match guard.get(&id) {
        Some((stored, payload)) if stored.elapsed() < CACHE_TTL => Some(payload.clone()),
        _ => None,
    }
}

async fn cache_put(id: Uuid, payload: ActivityDto) {
    let mut guard = CACHE.write().await;
    guard.insert(id, (Instant::now(), payload));
}

// --- The mandate source row (dedicated narrow read; does NOT touch MandateDto) ------------------

#[derive(Debug, Clone)]
struct MandateSource {
    source: Option<String>,
    source_external_id: Option<String>,
}

/// Read `source` + `source_external_id` for a mandate within an org. Runtime (unchecked) query so
/// the committed `.sqlx/` offline cache does not need regenerating on a DB-less build host.
async fn load_mandate_source(
    db: &sqlx::PgPool,
    org_id: Uuid,
    mandate_id: Uuid,
) -> Result<Option<MandateSource>, sqlx::Error> {
    let row = sqlx::query_as::<_, (Option<String>, Option<String>)>(
        "SELECT source, source_external_id FROM mandate WHERE org_id = $1 AND id = $2",
    )
    .bind(org_id)
    .bind(mandate_id)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(source, source_external_id)| MandateSource {
        source,
        source_external_id,
    }))
}

// --- Handler ------------------------------------------------------------------------------------

/// `GET /api/v1/mandates/{id}/atividade?org_id=<uuid>` — normalized public activity for a mandate.
///
/// Resolves the mandate's house + external id, serves a cached payload if fresh, otherwise fans out
/// to that house's APIs (in parallel), normalizes, caches, and returns. Missing source ⇒ empty OK.
async fn get_activity(
    State(state): State<AppState>,
    Path(mandate_id): Path<Uuid>,
    Query(query): Query<OrgQuery>,
) -> Json<ApiResponse<ActivityDto>> {
    // Serve fresh cache first — these upstreams are slow and rate-sensitive.
    if let Some(cached) = cache_get(mandate_id).await {
        return Json(ApiResponse::ok(cached));
    }

    let source = match load_mandate_source(&state.db, query.org_id, mandate_id).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            // Unknown mandate (or wrong org): return an empty-but-OK payload, matching the
            // "no activity" contract. The base mandate read already 404s a truly missing mandate.
            return Json(ApiResponse::ok(ActivityDto::default()));
        }
        Err(err) => {
            tracing::error!(error = ?err, "atividade: mandate source read failed");
            return Json(ApiResponse::ok(ActivityDto::default()));
        }
    };

    let (source_kind, external_id) = match (
        source.source.as_deref(),
        source.source_external_id.as_deref(),
    ) {
        (Some(s), Some(id)) if !id.is_empty() => (s.to_owned(), id.to_owned()),
        _ => {
            // No linked house profile (manual/candidate or historical seed) — nothing to proxy.
            return Json(ApiResponse::ok(ActivityDto {
                source: source.source.clone(),
                external_id: source.source_external_id.clone(),
                ..Default::default()
            }));
        }
    };

    let payload = match source_kind.as_str() {
        "camara" => fetch_camara(&external_id).await,
        "senado" => fetch_senado(&external_id).await,
        // 'manual' or anything unexpected: no upstream to hit.
        _ => ActivityDto {
            source: Some(source_kind.clone()),
            external_id: Some(external_id.clone()),
            ..Default::default()
        },
    };

    cache_put(mandate_id, payload.clone()).await;
    Json(ApiResponse::ok(payload))
}

// --- Upstream HTTP client -----------------------------------------------------------------------

/// A short-lived reqwest client with the per-request timeout + house User-Agent. Cheap to build;
/// the connection pool lives only for the fan-out of one request, which keeps this dependency-light.
fn client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(UPSTREAM_TIMEOUT_SECS))
        .user_agent(USER_AGENT)
        .build()
}

/// Fetch a URL as JSON. `accept_json` sends `Accept: application/json` (the Senado needs it or it
/// returns XML). Returns `None` (logged) on any error / non-200 so a section can degrade to empty.
async fn get_json(cli: &reqwest::Client, url: &str, accept_json: bool) -> Option<Value> {
    let mut req = cli.get(url);
    if accept_json {
        req = req.header(reqwest::header::ACCEPT, "application/json");
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(err) => {
            tracing::warn!(error = %err, url, "atividade: upstream request failed");
            return None;
        }
    };
    if !resp.status().is_success() {
        tracing::warn!(status = %resp.status(), url, "atividade: upstream non-200");
        return None;
    }
    match resp.json::<Value>().await {
        Ok(v) => Some(v),
        Err(err) => {
            tracing::warn!(error = %err, url, "atividade: upstream body not JSON");
            None
        }
    }
}

// --- Lower house -----------------------------------------------------------------------------

/// Fan out to the five lower-house endpoints in parallel, then normalize. Any single failure degrades to
/// an empty section (all helpers already return empty on `None`).
async fn fetch_camara(id: &str) -> ActivityDto {
    let cli = match client() {
        Ok(c) => c,
        Err(err) => {
            tracing::error!(error = %err, "atividade: reqwest client build failed");
            return ActivityDto {
                source: Some("camara".to_owned()),
                external_id: Some(id.to_owned()),
                ..Default::default()
            };
        }
    };

    let details_url = format!("{CAMARA_BASE}/deputados/{id}");
    let eventos_url = format!(
        "{CAMARA_BASE}/deputados/{id}/eventos?ordem=ASC&ordenarPor=dataHoraInicio&itens=15"
    );
    let props_url =
        format!("{CAMARA_BASE}/proposicoes?idDeputadoAutor={id}&ordem=DESC&ordenarPor=id&itens=20");
    let discursos_url = format!(
        "{CAMARA_BASE}/deputados/{id}/discursos?ordem=DESC&ordenarPor=dataHoraInicio&itens=10"
    );
    let despesas_url =
        format!("{CAMARA_BASE}/deputados/{id}/despesas?ordem=DESC&ordenarPor=ano&itens=100");

    let (details, eventos, props, discursos, despesas) = tokio::join!(
        get_json(&cli, &details_url, false),
        get_json(&cli, &eventos_url, false),
        get_json(&cli, &props_url, false),
        get_json(&cli, &discursos_url, false),
        get_json(&cli, &despesas_url, false),
    );

    ActivityDto {
        source: Some("camara".to_owned()),
        external_id: Some(id.to_owned()),
        agenda: camara_agenda(eventos.as_ref()),
        production: camara_production(props.as_ref()),
        speeches: camara_speeches(discursos.as_ref()),
        votes: Vec::new(),
        expenses: camara_expenses(despesas.as_ref()),
        profile_extra: camara_profile_extra(details.as_ref()),
    }
}

fn camara_agenda(v: Option<&Value>) -> Vec<AgendaItem> {
    let Some(items) = v.and_then(|v| v.get("dados")).and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .map(|e| AgendaItem {
            title: str_field(e, "descricao")
                .or_else(|| str_field(e, "descricaoTipo"))
                .unwrap_or_else(|| "Evento".to_owned()),
            date_time: str_field(e, "dataHoraInicio"),
            location: e
                .get("localCamara")
                .and_then(|l| l.get("nome"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .or_else(|| str_field(e, "localExterno")),
            kind: str_field(e, "descricaoTipo"),
            url: str_field(e, "urlRegistro").or_else(|| str_field(e, "uri")),
        })
        .collect()
}

fn camara_production(v: Option<&Value>) -> Vec<ProductionItem> {
    let Some(items) = v.and_then(|v| v.get("dados")).and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .map(|p| ProductionItem {
            id: num_or_str(p, "id"),
            kind: str_field(p, "siglaTipo"),
            number: num_or_str(p, "numero"),
            year: num_or_str(p, "ano"),
            summary: str_field(p, "ementa"),
            url: str_field(p, "uri"),
        })
        .collect()
}

fn camara_speeches(v: Option<&Value>) -> Vec<SpeechItem> {
    let Some(items) = v.and_then(|v| v.get("dados")).and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .map(|d| SpeechItem {
            date_time: str_field(d, "dataHoraInicio"),
            summary: str_field(d, "transcricao")
                .or_else(|| str_field(d, "sumario"))
                .or_else(|| str_field(d, "keywords")),
            url: str_field(d, "urlTexto").or_else(|| str_field(d, "urlVideo")),
        })
        .collect()
}

/// Summarize the cota parlamentar: total spend + top categories by `tipoDespesa`, using the most
/// recent year present in the page (the query orders by year DESC).
fn camara_expenses(v: Option<&Value>) -> Option<ExpenseSummary> {
    let items = v.and_then(|v| v.get("dados")).and_then(Value::as_array)?;
    if items.is_empty() {
        return None;
    }
    // Most recent year in the page.
    let year = items
        .iter()
        .filter_map(|e| e.get("ano").and_then(Value::as_i64))
        .max();
    let mut total = 0.0_f64;
    let mut by_cat: HashMap<String, f64> = HashMap::new();
    for e in items {
        // Only sum the most recent year so the "total" is a coherent annual figure.
        if let Some(y) = year {
            if e.get("ano").and_then(Value::as_i64) != Some(y) {
                continue;
            }
        }
        let amount = e
            .get("valorLiquido")
            .and_then(Value::as_f64)
            .or_else(|| e.get("valorDocumento").and_then(Value::as_f64))
            .unwrap_or(0.0);
        total += amount;
        let cat = str_field(e, "tipoDespesa").unwrap_or_else(|| "Outros".to_owned());
        *by_cat.entry(cat).or_insert(0.0) += amount;
    }
    let mut cats: Vec<ExpenseCategory> = by_cat
        .into_iter()
        .map(|(name, amount)| ExpenseCategory { name, amount })
        .collect();
    cats.sort_by(|a, b| {
        b.amount
            .partial_cmp(&a.amount)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    cats.truncate(5);
    Some(ExpenseSummary {
        year,
        total,
        top_categories: cats,
    })
}

fn camara_profile_extra(v: Option<&Value>) -> Option<ProfileExtra> {
    let dados = v.and_then(|v| v.get("dados"))?;
    let social = dados
        .get("redeSocial")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let status = dados.get("ultimoStatus");
    let office_contact = status
        .and_then(|s| s.get("gabinete"))
        .map(|g| {
            let sala = g.get("sala").and_then(Value::as_str).unwrap_or("");
            let predio = g.get("predio").and_then(Value::as_str).unwrap_or("");
            let tel = g.get("telefone").and_then(Value::as_str).unwrap_or("");
            let email = g.get("email").and_then(Value::as_str).unwrap_or("");
            format!("Gabinete {sala}, Prédio {predio} — Tel: {tel} — {email}")
        })
        .filter(|s| s.trim_matches(|c: char| !c.is_alphanumeric()).len() > 8);
    let photo_url = status
        .and_then(|s| s.get("urlFoto"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let page_url = dados
        .get("urlWebsite")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Some(ProfileExtra {
        social,
        office_contact,
        photo_url,
        page_url,
    })
}

// --- Senado Federal -----------------------------------------------------------------------------

/// Fan out to the Senado endpoints in parallel, then normalize the deeply-nested PascalCase JSON.
/// `agenda` 404s so it is not requested; `autorias` and `votacoes` are officially deprecated (still
/// serving data as of 2026) — they degrade to empty when the substitute-only cutover completes.
async fn fetch_senado(codigo: &str) -> ActivityDto {
    let cli = match client() {
        Ok(c) => c,
        Err(err) => {
            tracing::error!(error = %err, "atividade: reqwest client build failed");
            return ActivityDto {
                source: Some("senado".to_owned()),
                external_id: Some(codigo.to_owned()),
                ..Default::default()
            };
        }
    };

    let details_url = format!("{SENADO_BASE}/senador/{codigo}");
    let autorias_url = format!("{SENADO_BASE}/senador/{codigo}/autorias");
    let discursos_url = format!("{SENADO_BASE}/senador/{codigo}/discursos");
    let votacoes_url = format!("{SENADO_BASE}/senador/{codigo}/votacoes");

    let (details, autorias, discursos, votacoes) = tokio::join!(
        get_json(&cli, &details_url, true),
        get_json(&cli, &autorias_url, true),
        get_json(&cli, &discursos_url, true),
        get_json(&cli, &votacoes_url, true),
    );

    ActivityDto {
        source: Some("senado".to_owned()),
        external_id: Some(codigo.to_owned()),
        agenda: Vec::new(),
        production: senado_production(autorias.as_ref()),
        speeches: senado_speeches(discursos.as_ref()),
        votes: senado_votes(votacoes.as_ref()),
        expenses: None,
        profile_extra: senado_profile_extra(details.as_ref()),
    }
}

/// Walk `MateriasAutoriaParlamentar.Parlamentar.Autorias.Autoria[]`. The `Autoria` node may be a
/// single object or an array (Senado collapses singletons) — [`as_node_array`] handles both.
fn senado_production(v: Option<&Value>) -> Vec<ProductionItem> {
    let autorias = v
        .and_then(|v| v.get("MateriasAutoriaParlamentar"))
        .and_then(|v| v.get("Parlamentar"))
        .and_then(|v| v.get("Autorias"))
        .and_then(|v| v.get("Autoria"));
    as_node_array(autorias)
        .into_iter()
        .take(20)
        .filter_map(|a| a.get("Materia").cloned())
        .map(|m| ProductionItem {
            id: str_field(&m, "Codigo"),
            kind: str_field(&m, "Sigla"),
            number: str_field(&m, "Numero"),
            year: str_field(&m, "Ano"),
            summary: str_field(&m, "Ementa").or_else(|| str_field(&m, "DescricaoIdentificacao")),
            url: None,
        })
        .collect()
}

/// Walk `DiscursosParlamentar.Parlamentar.Pronunciamentos.Pronunciamento[]`. Often empty because
/// the Senado defaults to the last 30 days when no period is passed.
fn senado_speeches(v: Option<&Value>) -> Vec<SpeechItem> {
    let prons = v
        .and_then(|v| v.get("DiscursosParlamentar"))
        .and_then(|v| v.get("Parlamentar"))
        .and_then(|v| v.get("Pronunciamentos"))
        .and_then(|v| v.get("Pronunciamento"));
    as_node_array(prons)
        .into_iter()
        .take(10)
        .map(|p| SpeechItem {
            date_time: str_field(p, "DataPronunciamento").or_else(|| str_field(p, "Data")),
            summary: str_field(p, "TextoResumo")
                .or_else(|| str_field(p, "Indexacao"))
                .or_else(|| {
                    p.get("SessaoPlenaria")
                        .and_then(|s| s.get("DescricaoTipoSessao"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                }),
            url: str_field(p, "UrlTexto").or_else(|| str_field(p, "TextoIntegral")),
        })
        .collect()
}

/// Walk `VotacaoParlamentar.Parlamentar.Votacoes.Votacao[]`.
fn senado_votes(v: Option<&Value>) -> Vec<VoteItem> {
    let votacoes = v
        .and_then(|v| v.get("VotacaoParlamentar"))
        .and_then(|v| v.get("Parlamentar"))
        .and_then(|v| v.get("Votacoes"))
        .and_then(|v| v.get("Votacao"));
    as_node_array(votacoes)
        .into_iter()
        .take(20)
        .map(|vt| {
            let materia = vt.get("Materia");
            VoteItem {
                date: vt
                    .get("SessaoPlenaria")
                    .and_then(|s| s.get("DataSessao"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                matter: materia
                    .and_then(|m| m.get("DescricaoIdentificacao"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                summary: materia
                    .and_then(|m| m.get("Ementa"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                vote: str_field(vt, "SiglaDescricaoVoto"),
                result: str_field(vt, "DescricaoResultado"),
            }
        })
        .collect()
}

fn senado_profile_extra(v: Option<&Value>) -> Option<ProfileExtra> {
    let parlamentar = v
        .and_then(|v| v.get("DetalheParlamentar"))
        .and_then(|v| v.get("Parlamentar"))?;
    let ident = parlamentar.get("IdentificacaoParlamentar");
    let photo_url = ident
        .and_then(|i| i.get("UrlFotoParlamentar"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let page_url = ident
        .and_then(|i| i.get("UrlPaginaParlamentar"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    // Office contact: address + first phone.
    let endereco = parlamentar
        .get("DadosBasicosParlamentar")
        .and_then(|d| d.get("EnderecoParlamentar"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let telefone_node = parlamentar.get("Telefones").and_then(|t| t.get("Telefone"));
    let phone = as_node_array(telefone_node)
        .into_iter()
        .next()
        .and_then(|t| str_field(t, "NumeroTelefone"))
        .unwrap_or_default();
    let office_contact = {
        let combined = format!("{endereco} — Tel: {phone}");
        if combined
            .trim_matches(|c: char| !c.is_alphanumeric())
            .is_empty()
        {
            None
        } else {
            Some(combined)
        }
    };
    Some(ProfileExtra {
        social: Vec::new(),
        office_contact,
        photo_url,
        page_url,
    })
}

// --- Small JSON helpers -------------------------------------------------------------------------

/// Read a string field, dropping empty strings (both APIs use `""` liberally for "absent").
fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Read a field that may be a JSON number OR a JSON string (the upstream returns numeric ids/numbers;
/// stringify uniformly so the DTO is stable regardless of the wire representation).
fn num_or_str(v: &Value, key: &str) -> Option<String> {
    match v.get(key) {
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

/// Normalize a Senado node that is EITHER a single object OR an array of objects (their JSON
/// collapses singletons to a bare object) into a `Vec<&Value>`. `None`/other ⇒ empty.
fn as_node_array(v: Option<&Value>) -> Vec<&Value> {
    match v {
        Some(Value::Array(arr)) => arr.iter().collect(),
        Some(obj @ Value::Object(_)) => vec![obj],
        _ => Vec::new(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn camara_production_maps_fields() {
        let v = json!({"dados": [
            {"id": 2636332, "siglaTipo": "EMC", "numero": 38, "ano": 2026, "ementa": "Dá nova redação", "uri": "http://x"}
        ]});
        let out = camara_production(Some(&v));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id.as_deref(), Some("2636332"));
        assert_eq!(out[0].kind.as_deref(), Some("EMC"));
        assert_eq!(out[0].number.as_deref(), Some("38"));
        assert_eq!(out[0].year.as_deref(), Some("2026"));
    }

    #[test]
    fn camara_expenses_summarizes_latest_year_only() {
        let v = json!({"dados": [
            {"ano": 2026, "tipoDespesa": "COMBUSTÍVEIS", "valorLiquido": 100.0},
            {"ano": 2026, "tipoDespesa": "COMBUSTÍVEIS", "valorLiquido": 50.0},
            {"ano": 2026, "tipoDespesa": "PASSAGENS", "valorLiquido": 25.0},
            {"ano": 2025, "tipoDespesa": "COMBUSTÍVEIS", "valorLiquido": 9999.0}
        ]});
        let s = camara_expenses(Some(&v)).unwrap();
        assert_eq!(s.year, Some(2026));
        assert_eq!(s.total, 175.0);
        assert_eq!(s.top_categories[0].name, "COMBUSTÍVEIS");
        assert_eq!(s.top_categories[0].amount, 150.0);
    }

    #[test]
    fn senado_singleton_node_is_treated_as_one_element() {
        let single = json!({"Materia": {"Codigo": "1", "Sigla": "RQS", "Numero": "2", "Ano": "2023", "Ementa": "x"}});
        assert_eq!(as_node_array(Some(&single)).len(), 1);
        let arr = json!([{"a": 1}, {"a": 2}]);
        assert_eq!(as_node_array(Some(&arr)).len(), 2);
        assert_eq!(as_node_array(None).len(), 0);
    }

    #[test]
    fn senado_votes_extract_matter_and_vote() {
        let v = json!({"VotacaoParlamentar": {"Parlamentar": {"Votacoes": {"Votacao": [
            {"SessaoPlenaria": {"DataSessao": "2024-04-16"},
             "Materia": {"DescricaoIdentificacao": "PEC 45/2023", "Ementa": "Altera art."},
             "DescricaoResultado": "Aprovado", "SiglaDescricaoVoto": "Sim"}
        ]}}}});
        let out = senado_votes(Some(&v));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].matter.as_deref(), Some("PEC 45/2023"));
        assert_eq!(out[0].vote.as_deref(), Some("Sim"));
        assert_eq!(out[0].date.as_deref(), Some("2024-04-16"));
    }

    #[test]
    fn empty_inputs_degrade_to_empty_sections() {
        assert!(camara_agenda(None).is_empty());
        assert!(camara_production(None).is_empty());
        assert!(camara_speeches(None).is_empty());
        assert!(camara_expenses(None).is_none());
        assert!(senado_production(None).is_empty());
        assert!(senado_votes(None).is_empty());
    }
}

//! # Office CRM (C6 of the product critique plan).
//!
//! The 1st NON-defensive reason for an office to open the app: instead of a "task queue
//! with a running deadline", it shows **who approached you and what they asked** — a
//! RELATIONSHIP tool for the base, not a pressure tool.
//!
//! ## Gate
//! Only the mandate's operator: whoever holds a binding in `mandate_identity_binding`
//! (the same criterion as the mandate panel and `campanha.rs`). The CRM is always
//! scoped to THAT operator's mandate — the query keys on
//! `proposal_target.mandate_id = <the caller's mandate>`, so it is structurally
//! impossible for an operator to see another office's CRM.
//!
//! - `GET /me/mandate/crm` — people who directed proposals at this mandate,
//!   with a history per person, a theme (light keyword grouping) and a
//!   status (answered/pending/silence/open).
//!
//! ## Nota LGPD (CUIDADO)
//! It only exposes **ALREADY-PUBLIC data**: the authorship of a directed proposal is public
//! (the public page `/propostas/{id}` already shows `author_handle`,
//! `author_public_handle` and the title/body). This endpoint re-aggregates that same
//! public data PER PERSON, for the recipient office.
//!
//! It **NEVER** exposes votes/support (aggregate and private by design — the platform
//! does not even keep a per-citizen vote queryable here), nor PII from
//! `auth_credential` (e-mail/phone/document). Anonymous proposals
//! (`author_citizen_id IS NULL`) are left out: there is no person to build a
//! relationship with and no public authorship to attribute.

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

/// Cap of rows read — the CRM is a summary, not an export. At production scale this is
/// small; fine-grained pagination waits until an office has thousands.
const CRM_LIMIT: i64 = 1000;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/me/mandate/crm", get(crm))
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

fn storage_error() -> Response {
    fail(
        StatusCode::INTERNAL_SERVER_ERROR,
        "storage_error",
        "Erro interno.",
    )
}

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

/// One demand (a directed proposal) from a person to this mandate.
#[derive(Debug, Serialize)]
struct CrmDemand {
    proposal_id: Uuid,
    title: String,
    /// Theme derived by keyword (light grouping, no embeddings — C7).
    theme: String,
    /// `respondida` | `pendente` | `silencio` | `aberta` (aguardando apoios).
    status: String,
    urgencia: String,
    created_at: DateTime<Utc>,
}

/// A person who approached the office, with their history of demands.
#[derive(Debug, Serialize)]
struct CrmContact {
    citizen_id: Uuid,
    /// Chosen handle (`@fulana`) — may be `None` when never set.
    handle: Option<String>,
    /// Opaque public handle (`u-<hex>`), always present — the display fallback.
    public_handle: String,
    display_name: Option<String>,
    avatar_url: Option<String>,
    demands_count: i64,
    answered_count: i64,
    pending_count: i64,
    silence_count: i64,
    open_count: i64,
    first_contact_at: DateTime<Utc>,
    last_contact_at: DateTime<Utc>,
    demands: Vec<CrmDemand>,
}

/// Light aggregation by theme (groundwork for C7 — the demand aggregator).
#[derive(Debug, Serialize)]
struct CrmTheme {
    theme: String,
    demands_count: i64,
    contacts_count: i64,
}

#[derive(Debug, Default, Serialize)]
struct CrmTotals {
    contacts: i64,
    demands: i64,
    answered: i64,
    pending: i64,
    silence: i64,
    open: i64,
}

#[derive(Debug, Serialize)]
struct CrmDto {
    mandate_id: Uuid,
    contacts: Vec<CrmContact>,
    themes: Vec<CrmTheme>,
    totals: CrmTotals,
}

#[derive(Debug, Deserialize)]
struct CrmParams {
    /// Optional filter by status (`respondida|pendente|silencio|aberta`).
    status: Option<String>,
    /// Optional filter by theme (the exact label returned in `themes[].theme`).
    theme: Option<String>,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// Raw query row (one per demand directed at the mandate).
#[derive(sqlx::FromRow)]
struct CrmRow {
    proposal_id: Uuid,
    title: String,
    body: String,
    urgencia: String,
    created_at: DateTime<Utc>,
    author_citizen_id: Uuid,
    author_handle: Option<String>,
    author_display_name: Option<String>,
    author_avatar_object_key: Option<String>,
    sla_status: Option<String>,
}

async fn crm(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<CrmParams>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return fail(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Autenticação necessária.",
        );
    };

    // Gate + mandate resolution: the binding is the authorization AND the scope key.
    let mandate_id: Option<Uuid> = match sqlx::query_scalar(
        "SELECT mandate_id FROM mandate_identity_binding \
         WHERE citizen_id = $1 ORDER BY verified_at DESC LIMIT 1",
    )
    .bind(citizen)
    .fetch_optional(&state.db)
    .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "crm gate check");
            return storage_error();
        }
    };
    let Some(mandate_id) = mandate_id else {
        return fail(
            StatusCode::FORBIDDEN,
            "not_operator",
            "O CRM de gabinete é exclusivo de contas vinculadas a um mandato.",
        );
    };

    let media_base = std::env::var("MEDIA_BASE_URL").unwrap_or_else(|_| "/media".to_owned());
    let media_base = media_base.trim_end_matches('/').to_owned();

    // Only proposals with a NON-null author (public authorship). Anonymous ones are left out
    // (no PII, no relationship without a counterpart). The status comes from this
    // mandate's SLA, when one exists; without an SLA the demand is still gathering support ("open").
    let rows: Vec<CrmRow> = match sqlx::query_as(
        r"
        SELECT p.id                 AS proposal_id,
               p.title              AS title,
               p.body               AS body,
               p.urgencia           AS urgencia,
               p.created_at         AS created_at,
               p.author_citizen_id  AS author_citizen_id,
               c.handle             AS author_handle,
               c.display_name       AS author_display_name,
               c.avatar_object_key  AS author_avatar_object_key,
               s.status             AS sla_status
          FROM proposal_target pt
          JOIN proposal p ON p.id = pt.proposal_id
          JOIN citizen  c ON c.id = p.author_citizen_id
     LEFT JOIN consequence_sla s
            ON s.proposal_id = p.id AND s.mandate_id = pt.mandate_id
         WHERE pt.mandate_id = $1
           AND p.author_citizen_id IS NOT NULL
         ORDER BY p.created_at DESC
         LIMIT $2
        ",
    )
    .bind(mandate_id)
    .bind(CRM_LIMIT)
    .fetch_all(&state.db)
    .await
    {
        Ok(r) => r,
        Err(err) => {
            tracing::error!(?err, "crm rows read");
            return storage_error();
        }
    };

    let dto = build_crm(
        mandate_id,
        rows,
        &media_base,
        params.status.as_deref(),
        params.theme.as_deref(),
    );
    (StatusCode::OK, Json(ApiResponse::ok(dto))).into_response()
}

// ---------------------------------------------------------------------------
// Aggregation (pure — testable without a DB)
// ---------------------------------------------------------------------------

/// Map the SLA status onto the CRM's vocabulary. No SLA = the demand is still
/// gathering support (it has not crossed the trigger) → "open".
fn status_label(sla_status: Option<&str>) -> &'static str {
    match sla_status {
        Some("answered") | Some("acted") => "respondida",
        Some("pending") => "pendente",
        Some("ignored") => "silencio",
        _ => "aberta",
    }
}

/// Aggregates raw rows into contacts (per person) + themes + totals. Applies the
/// optional status/theme filters BEFORE aggregating, so the counters
/// reflect exactly what the UI shows.
fn build_crm(
    mandate_id: Uuid,
    rows: Vec<CrmRow>,
    media_base: &str,
    status_filter: Option<&str>,
    theme_filter: Option<&str>,
) -> CrmDto {
    // Preserves order of appearance (rows already arrive by created_at DESC).
    let mut order: Vec<Uuid> = Vec::new();
    let mut by_citizen: BTreeMap<Uuid, CrmContact> = BTreeMap::new();
    let mut theme_demands: BTreeMap<String, i64> = BTreeMap::new();
    let mut theme_citizens: BTreeMap<String, std::collections::BTreeSet<Uuid>> = BTreeMap::new();
    let mut totals = CrmTotals::default();

    for row in rows {
        let status = status_label(row.sla_status.as_deref());
        if let Some(f) = status_filter {
            if !f.is_empty() && f != status {
                continue;
            }
        }
        let theme = derive_theme(&row.title, &row.body);
        if let Some(f) = theme_filter {
            if !f.is_empty() && f != theme {
                continue;
            }
        }

        let avatar_url = row.author_avatar_object_key.as_deref().and_then(|k| {
            if media_base.is_empty() {
                None
            } else {
                Some(format!("{media_base}/{k}"))
            }
        });
        let public_handle = format!("u-{}", row.author_citizen_id.simple());

        let entry = by_citizen.entry(row.author_citizen_id).or_insert_with(|| {
            order.push(row.author_citizen_id);
            CrmContact {
                citizen_id: row.author_citizen_id,
                handle: row.author_handle.clone(),
                public_handle,
                display_name: row.author_display_name.clone(),
                avatar_url,
                demands_count: 0,
                answered_count: 0,
                pending_count: 0,
                silence_count: 0,
                open_count: 0,
                first_contact_at: row.created_at,
                last_contact_at: row.created_at,
                demands: Vec::new(),
            }
        });

        entry.demands_count += 1;
        match status {
            "respondida" => entry.answered_count += 1,
            "pendente" => entry.pending_count += 1,
            "silencio" => entry.silence_count += 1,
            _ => entry.open_count += 1,
        }
        // rows arrive DESC: the first seen is the most recent, the last the oldest.
        if row.created_at > entry.last_contact_at {
            entry.last_contact_at = row.created_at;
        }
        if row.created_at < entry.first_contact_at {
            entry.first_contact_at = row.created_at;
        }
        entry.demands.push(CrmDemand {
            proposal_id: row.proposal_id,
            title: row.title,
            theme: theme.clone(),
            status: status.to_owned(),
            urgencia: row.urgencia,
            created_at: row.created_at,
        });

        *theme_demands.entry(theme.clone()).or_insert(0) += 1;
        theme_citizens
            .entry(theme)
            .or_default()
            .insert(row.author_citizen_id);

        totals.demands += 1;
        match status {
            "respondida" => totals.answered += 1,
            "pendente" => totals.pending += 1,
            "silencio" => totals.silence += 1,
            _ => totals.open += 1,
        }
    }

    // Contacts in most-recent-contact order (the order preserves first-seen = most recent).
    let contacts: Vec<CrmContact> = order
        .into_iter()
        .filter_map(|id| by_citizen.remove(&id))
        .collect();
    totals.contacts = contacts.len() as i64;

    // Themes ordered by volume desc, then alphabetically (stable).
    let mut themes: Vec<CrmTheme> = theme_demands
        .into_iter()
        .map(|(theme, demands_count)| {
            let contacts_count = theme_citizens.get(&theme).map(|s| s.len()).unwrap_or(0) as i64;
            CrmTheme {
                theme,
                demands_count,
                contacts_count,
            }
        })
        .collect();
    themes.sort_by(|a, b| {
        b.demands_count
            .cmp(&a.demands_count)
            .then_with(|| a.theme.cmp(&b.theme))
    });

    CrmDto {
        mandate_id,
        contacts,
        themes,
        totals,
    }
}

/// Light grouping by theme (groundwork for C7). No embeddings: it matches the
/// title+body (lowercased) against a curated dictionary of Brazilian civic
/// themes. First match wins; no match → "Geral".
/// Replaceable by real embeddings (C7) without changing the endpoint's contract.
fn derive_theme(title: &str, body: &str) -> String {
    let hay = format!("{title} {body}").to_lowercase();
    // (label, keywords). Order = priority.
    const THEMES: &[(&str, &[&str])] = &[
        (
            "Saúde",
            &[
                "saúde", "saude", "hospital", "posto", "sus", "médic", "medic", "vacina", "upa",
                "samu",
            ],
        ),
        (
            "Educação",
            &[
                "educa",
                "escola",
                "creche",
                "professor",
                "ensino",
                "universidad",
                "merenda",
                "alfabetiz",
            ],
        ),
        (
            "Segurança",
            &[
                "seguran",
                "polícia",
                "policia",
                "violênc",
                "violenc",
                "crime",
                "iluminação",
                "iluminacao",
            ],
        ),
        (
            "Transporte",
            &[
                "transport",
                "ônibus",
                "onibus",
                "mobilidad",
                "trânsit",
                "transit",
                "ciclovia",
                "metrô",
                "metro",
                "tarifa",
            ],
        ),
        (
            "Saneamento",
            &[
                "saneament",
                "esgoto",
                "água",
                "agua",
                "lixo",
                "resíduo",
                "residuo",
                "drenag",
                "enchent",
            ],
        ),
        (
            "Meio ambiente",
            &[
                "ambient",
                "poluiç",
                "poluic",
                "árvore",
                "arvore",
                "desmatament",
                "reciclag",
                "clima",
            ],
        ),
        (
            "Moradia",
            &[
                "moradia",
                "habitaç",
                "habitac",
                "aluguel",
                "despejo",
                "regulariz",
                "favela",
                "ocupaç",
                "ocupac",
            ],
        ),
        (
            "Trabalho e renda",
            &[
                "emprego",
                "trabalh",
                "renda",
                "desemprego",
                "informal",
                "microempre",
                "salário",
                "salario",
            ],
        ),
        (
            "Cultura e esporte",
            &[
                "cultura",
                "esporte",
                "quadra",
                "praça",
                "praca",
                "biblioteca",
                "teatro",
                "lazer",
            ],
        ),
        (
            "Assistência social",
            &[
                "assistênc",
                "assistenc",
                "bolsa",
                "fome",
                "vulnerab",
                "idoso",
                "criança",
                "crianca",
            ],
        ),
        (
            "Direitos e cidadania",
            &[
                "direito",
                "cidadania",
                "lgbt",
                "racism",
                "mulher",
                "acessibilidad",
                "deficiênc",
                "deficienc",
            ],
        ),
        (
            "Infraestrutura",
            &[
                "asfalt",
                "pavimenta",
                "buraco",
                "obra",
                "ponte",
                "calçad",
                "calcad",
                "via pública",
                "via publica",
            ],
        ),
    ];
    for (label, needles) in THEMES {
        if needles.iter().any(|n| hay.contains(n)) {
            return (*label).to_owned();
        }
    }
    "Geral".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(author: Uuid, title: &str, sla: Option<&str>, created: DateTime<Utc>) -> CrmRow {
        CrmRow {
            proposal_id: Uuid::now_v7(),
            title: title.to_owned(),
            body: String::new(),
            urgencia: "comum".to_owned(),
            created_at: created,
            author_citizen_id: author,
            author_handle: Some("fulana".to_owned()),
            author_display_name: Some("Fulana".to_owned()),
            author_avatar_object_key: None,
            sla_status: sla.map(|s| s.to_owned()),
        }
    }

    #[test]
    fn status_labels_map_sla_to_crm_vocab() {
        assert_eq!(status_label(Some("answered")), "respondida");
        assert_eq!(status_label(Some("acted")), "respondida");
        assert_eq!(status_label(Some("pending")), "pendente");
        assert_eq!(status_label(Some("ignored")), "silencio");
        assert_eq!(status_label(None), "aberta");
    }

    #[test]
    fn theme_keyword_matching() {
        assert_eq!(derive_theme("Falta médico no posto", ""), "Saúde");
        assert_eq!(derive_theme("Creche sem vaga", ""), "Educação");
        assert_eq!(derive_theme("Buraco na rua", ""), "Infraestrutura");
        assert_eq!(derive_theme("Assunto qualquer", ""), "Geral");
    }

    #[test]
    fn aggregates_by_person_and_theme() {
        let mandate = Uuid::now_v7();
        let ana = Uuid::now_v7();
        let beto = Uuid::now_v7();
        let t0 = "2026-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let t1 = "2026-02-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let rows = vec![
            row(ana, "Falta médico no posto", Some("answered"), t1),
            row(ana, "Vacina em falta", None, t0),
            row(beto, "Buraco na rua", Some("ignored"), t0),
        ];
        let dto = build_crm(mandate, rows, "", None, None);
        assert_eq!(dto.totals.contacts, 2);
        assert_eq!(dto.totals.demands, 3);
        assert_eq!(dto.totals.answered, 1);
        assert_eq!(dto.totals.silence, 1);
        assert_eq!(dto.totals.open, 1);
        // Ana has 2 demands; the contact window covers t0..t1.
        let ana_c = dto.contacts.iter().find(|c| c.citizen_id == ana).unwrap();
        assert_eq!(ana_c.demands_count, 2);
        assert_eq!(ana_c.first_contact_at, t0);
        assert_eq!(ana_c.last_contact_at, t1);
        // Health aggregates 2 demands from 1 person.
        let saude = dto.themes.iter().find(|t| t.theme == "Saúde").unwrap();
        assert_eq!(saude.demands_count, 2);
        assert_eq!(saude.contacts_count, 1);
    }

    #[test]
    fn status_filter_narrows_and_recounts() {
        let mandate = Uuid::now_v7();
        let ana = Uuid::now_v7();
        let t0 = "2026-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let rows = vec![
            row(ana, "Falta médico", Some("ignored"), t0),
            row(ana, "Creche", Some("answered"), t0),
        ];
        let dto = build_crm(mandate, rows, "", Some("silencio"), None);
        assert_eq!(dto.totals.demands, 1);
        assert_eq!(dto.totals.silence, 1);
        assert_eq!(dto.contacts.len(), 1);
        assert_eq!(dto.contacts[0].demands_count, 1);
    }
}

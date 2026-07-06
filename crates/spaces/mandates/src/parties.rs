//! Public read surface for the party catalog (migration 0204). Fase 2B do roadmap.
//!
//! Routes (merged into the gateway's `/api/v1` group via [`routes`]):
//! * `GET /api/v1/parties?org_id=<uuid>` — party directory ordered by mandate count DESC.
//! * `GET /api/v1/parties/{sigla}?org_id=<uuid>` — one party with its directories + admins.
//!
//! Design notes:
//! * Reads only. Writes (create party, invite admin, upsert directory) will land as a
//!   separate mutating surface once the admin UI is spec'd.
//! * SQL uses runtime (`sqlx::query_as`) unchecked queries — same pattern as
//!   `parlamentar_activity.rs` — so the committed `.sqlx/` offline cache doesn't need to be
//!   regenerated on a DB-less build host (SQLX_OFFLINE=1 build).
//! * `AdminBriefDto` intentionally exposes ONLY the citizen's `handle` and `display_name`;
//!   NEVER the citizen id or e-mail (privacy: this is a PUBLIC endpoint).
//! * When no rows exist yet (fresh org, or the migration ran but the seed hasn't) all
//!   endpoints degrade to empty lists — never an error.

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use dsoc_api_contract::envelope::ApiResponse;
use dsoc_app::AppState;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// DTOs (public shapes)
// ---------------------------------------------------------------------------

/// Public view of a party (list item). `mandate_count` is derived on the fly from
/// `mandate.party` so the UI can rank without a materialized column.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartyDto {
    pub sigla: String,
    pub name: String,
    pub tse_number: Option<i32>,
    pub logo_url: Option<String>,
    pub founded_year: Option<i32>,
    /// Mandates currently attributed to this sigla in the org (derived).
    pub mandate_count: i64,
}

/// Public view of a subnational directory of a party.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartyDirectoryDto {
    pub id: Uuid,
    pub party_sigla: String,
    pub esfera: String,
    pub uf: Option<String>,
    pub municipio: Option<String>,
    pub name: String,
    pub parent_directory_id: Option<Uuid>,
}

/// Public (privacy-safe) view of a party administrator. NEVER exposes the citizen's UUID
/// or e-mail — only their public @handle and display name, exactly like proposal authors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminBriefDto {
    /// User-chosen handle (`@fulana`). `None` when the citizen never picked one.
    pub public_handle: Option<String>,
    /// Friendly display name (may be `None` if the citizen never set it).
    pub display_name: Option<String>,
    /// `admin` | `moderador`.
    pub role: String,
    /// Scope: the directory id (NULL = administrator of the party at the national level).
    pub directory_id: Option<Uuid>,
}

/// Detail response: a party plus its directories and administrators.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartyDetailDto {
    #[serde(flatten)]
    pub party: PartyDto,
    pub directories: Vec<PartyDirectoryDto>,
    pub administrators: Vec<AdminBriefDto>,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct OrgQuery {
    org_id: Uuid,
}

/// Mount the party catalog routes. Public / read-only; no `CallerId` extraction needed.
/// Merged INTO the gateway's `/api/v1` group so public paths are `/api/v1/parties`
/// and `/api/v1/parties/{sigla}`.
pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/parties", get(list_parties))
        .route("/parties/{sigla}", get(get_party))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn list_parties(
    State(state): State<AppState>,
    Query(query): Query<OrgQuery>,
) -> Json<ApiResponse<Vec<PartyDto>>> {
    match load_parties(&state.db, query.org_id).await {
        Ok(list) => Json(ApiResponse::ok(list)),
        Err(err) => {
            tracing::error!(error = ?err, "parties: list failed");
            // Never leak the DB error to the caller; empty list is the safe public fallback.
            Json(ApiResponse::ok(Vec::new()))
        }
    }
}

async fn get_party(
    State(state): State<AppState>,
    Path(sigla): Path<String>,
    Query(query): Query<OrgQuery>,
) -> Json<ApiResponse<Option<PartyDetailDto>>> {
    match load_party_detail(&state.db, query.org_id, &sigla).await {
        Ok(detail) => Json(ApiResponse::ok(detail)),
        Err(err) => {
            tracing::error!(error = ?err, sigla, "parties: detail failed");
            Json(ApiResponse::ok(None))
        }
    }
}

// ---------------------------------------------------------------------------
// SQL (runtime queries — no `.sqlx/` cache regeneration needed)
// ---------------------------------------------------------------------------

async fn load_parties(db: &sqlx::PgPool, org_id: Uuid) -> Result<Vec<PartyDto>, sqlx::Error> {
    // LEFT JOIN so a freshly-created party with zero mandates still appears (count = 0).
    // ORDER BY mandate_count DESC, then sigla for a stable tie-break.
    let rows: Vec<(String, String, Option<i32>, Option<String>, Option<i32>, Option<i64>)> =
        sqlx::query_as(
            r"
            SELECT p.sigla,
                   p.name,
                   p.tse_number,
                   p.logo_url,
                   p.founded_year,
                   COUNT(m.id) AS mandate_count
              FROM party p
              LEFT JOIN mandate m
                     ON m.org_id = p.org_id
                    AND m.party  = p.sigla
             WHERE p.org_id = $1
             GROUP BY p.sigla, p.name, p.tse_number, p.logo_url, p.founded_year
             ORDER BY COUNT(m.id) DESC, p.sigla ASC
            ",
        )
        .bind(org_id)
        .fetch_all(db)
        .await?;

    Ok(rows
        .into_iter()
        .map(|(sigla, name, tse, logo, year, count)| PartyDto {
            sigla,
            name,
            tse_number: tse,
            logo_url: logo,
            founded_year: year,
            mandate_count: count.unwrap_or(0),
        })
        .collect())
}

async fn load_party_detail(
    db: &sqlx::PgPool,
    org_id: Uuid,
    sigla: &str,
) -> Result<Option<PartyDetailDto>, sqlx::Error> {
    // 1) The party row + derived mandate count.
    let party_row: Option<(String, String, Option<i32>, Option<String>, Option<i32>, Option<i64>)> =
        sqlx::query_as(
            r"
            SELECT p.sigla,
                   p.name,
                   p.tse_number,
                   p.logo_url,
                   p.founded_year,
                   COUNT(m.id) AS mandate_count
              FROM party p
              LEFT JOIN mandate m
                     ON m.org_id = p.org_id
                    AND m.party  = p.sigla
             WHERE p.org_id = $1 AND p.sigla = $2
             GROUP BY p.sigla, p.name, p.tse_number, p.logo_url, p.founded_year
            ",
        )
        .bind(org_id)
        .bind(sigla)
        .fetch_optional(db)
        .await?;

    let Some((s, name, tse, logo, year, count)) = party_row else {
        return Ok(None);
    };
    let party = PartyDto {
        sigla: s,
        name,
        tse_number: tse,
        logo_url: logo,
        founded_year: year,
        mandate_count: count.unwrap_or(0),
    };

    // 2) Directories (any esfera). Ordered federal → estadual → municipal by natural sort.
    let dir_rows: Vec<(Uuid, String, String, Option<String>, Option<String>, String, Option<Uuid>)> =
        sqlx::query_as(
            r"
            SELECT id, party_sigla, esfera, uf, municipio, name, parent_directory_id
              FROM party_directory
             WHERE org_id = $1 AND party_sigla = $2
             ORDER BY CASE esfera
                        WHEN 'federal'   THEN 0
                        WHEN 'estadual'  THEN 1
                        WHEN 'municipal' THEN 2
                        ELSE 3
                      END,
                      uf ASC NULLS FIRST,
                      municipio ASC NULLS FIRST,
                      name ASC
            ",
        )
        .bind(org_id)
        .bind(sigla)
        .fetch_all(db)
        .await?;

    let directories = dir_rows
        .into_iter()
        .map(|(id, party_sigla, esfera, uf, municipio, name, parent)| PartyDirectoryDto {
            id,
            party_sigla,
            esfera,
            uf,
            municipio,
            name,
            parent_directory_id: parent,
        })
        .collect();

    // 3) Administrators. Privacy filter: only accepted admins are exposed publicly.
    // Only expose (handle, display_name, role, directory scope). Never the citizen id.
    let admin_rows: Vec<(Option<String>, Option<String>, String, Option<Uuid>)> = sqlx::query_as(
        r"
        SELECT c.handle, c.display_name, pa.role, pa.directory_id
          FROM party_administrator pa
          JOIN citizen c ON c.id = pa.citizen_id
         WHERE pa.org_id = $1 AND pa.party_sigla = $2 AND pa.accepted_at IS NOT NULL
         ORDER BY pa.directory_id NULLS FIRST, pa.created_at ASC
        ",
    )
    .bind(org_id)
    .bind(sigla)
    .fetch_all(db)
    .await?;

    let administrators = admin_rows
        .into_iter()
        .map(|(handle, display, role, directory_id)| AdminBriefDto {
            public_handle: handle,
            display_name: display,
            role,
            directory_id,
        })
        .collect();

    Ok(Some(PartyDetailDto {
        party,
        directories,
        administrators,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn party_dto_serializes_expected_keys() {
        let dto = PartyDto {
            sigla: "PT".to_owned(),
            name: "Partido dos Trabalhadores".to_owned(),
            tse_number: Some(13),
            logo_url: None,
            founded_year: Some(1980),
            mandate_count: 42,
        };
        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["sigla"], "PT");
        assert_eq!(json["mandate_count"], 42);
        assert_eq!(json["tse_number"], 13);
    }

    #[test]
    fn admin_brief_never_carries_citizen_id_or_email() {
        // Structural guard: `AdminBriefDto` has only the four public fields. Any addition
        // that leaks the citizen id would fail this test at compile time via field access.
        let brief = AdminBriefDto {
            public_handle: Some("ana".to_owned()),
            display_name: Some("Ana Silva".to_owned()),
            role: "admin".to_owned(),
            directory_id: None,
        };
        let json = serde_json::to_value(&brief).unwrap();
        // Positive: expected fields present.
        assert!(json.get("public_handle").is_some());
        assert!(json.get("role").is_some());
        // Negative: forbidden fields absent.
        assert!(json.get("citizen_id").is_none());
        assert!(json.get("email").is_none());
    }

    #[test]
    fn party_detail_flattens_party_fields() {
        let detail = PartyDetailDto {
            party: PartyDto {
                sigla: "PSOL".to_owned(),
                name: "PSOL".to_owned(),
                tse_number: Some(50),
                logo_url: None,
                founded_year: None,
                mandate_count: 3,
            },
            directories: Vec::new(),
            administrators: Vec::new(),
        };
        let json = serde_json::to_value(&detail).unwrap();
        // `#[serde(flatten)]` should hoist `sigla`/`name` to the top level.
        assert_eq!(json["sigla"], "PSOL");
        assert_eq!(json["mandate_count"], 3);
        assert!(json["directories"].is_array());
        assert!(json["administrators"].is_array());
    }
}

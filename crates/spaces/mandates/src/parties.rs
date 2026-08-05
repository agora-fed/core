//! Public read surface for the party catalog (migration 0204). Roadmap phase 2B.
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
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use dsoc_api_contract::envelope::ApiResponse;
use dsoc_app::{AppState, CallerId};
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
    pub website: Option<String>,
    pub founded_year: Option<i32>,
    /// Mandates currently attributed to this sigla in the org (derived, non-hidden).
    pub mandate_count: i64,
    /// Count per sphere — spares the page from downloading every mandate to derive it.
    pub federal_count: i64,
    pub estadual_count: i64,
    pub municipal_count: i64,
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

/// Public detail of ONE chapter (subnational party directory) — the payload of
/// the chapter page. English contract for new surfaces (ADR-0013): `level`
/// maps the stored `esfera` (`federal|estadual|municipal` →
/// `national|state|municipal`). Same privacy rule as the party detail:
/// administrators expose ONLY handle/display_name — never citizen id or e-mail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChapterDto {
    pub id: Uuid,
    /// The party sigla ("PT", "PCdoB", ...) — case as stored.
    pub party_short_name: String,
    pub party_name: String,
    pub party_logo_url: Option<String>,
    /// `national` | `state` | `municipal`.
    pub level: String,
    /// UF code (present for state and municipal chapters).
    pub state: Option<String>,
    pub municipality: Option<String>,
    pub name: String,
    pub parent_id: Option<Uuid>,
    /// Administrators scoped to THIS chapter (accepted only; privacy-safe).
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
        // Write surface (0.37.0 — Phase 2.1): create/remove subnational directories.
        // Gate: platform admin OR the party's national admin (party_write_authorized).
        .route("/parties/{sigla}/directories", post(create_directory))
        .route(
            "/parties/{sigla}/directories/{id}",
            axum::routing::delete(delete_directory),
        )
        // Members derived from the directory: the party's mandates in that territory.
        // Public (read-only) — the same territorial logic PartyDetail used to derive client-side.
        .route(
            "/parties/{sigla}/directories/{id}/members",
            get(list_directory_members),
        )
        // One chapter (English contract, ADR-0013): the public chapter-page payload.
        .route("/parties/{sigla}/chapters/{id}", get(get_chapter))
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

/// `GET /api/v1/parties/{sigla}/chapters/{id}?org_id=` — one chapter with its
/// scoped administrators. `null` data when the id does not exist under that
/// party/org (never an error for a miss — same posture as `get_party`).
async fn get_chapter(
    State(state): State<AppState>,
    Path((sigla, id)): Path<(String, Uuid)>,
    Query(query): Query<OrgQuery>,
) -> Json<ApiResponse<Option<ChapterDto>>> {
    match load_chapter(&state.db, query.org_id, &sigla, id).await {
        Ok(chapter) => Json(ApiResponse::ok(chapter)),
        Err(err) => {
            tracing::error!(error = ?err, sigla, %id, "parties: chapter detail failed");
            Json(ApiResponse::ok(None))
        }
    }
}

/// Row of the chapter+party join in [`load_chapter`]:
/// (party sigla, party name, logo, esfera, uf, municipio, chapter name, parent id).
type ChapterRow = (
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    String,
    Option<Uuid>,
);

/// Map the stored `esfera` to the English `level` of the public contract.
fn esfera_to_level(esfera: &str) -> &'static str {
    match esfera {
        "federal" => "national",
        "estadual" => "state",
        _ => "municipal",
    }
}

async fn load_chapter(
    db: &sqlx::PgPool,
    org_id: Uuid,
    sigla: &str,
    id: Uuid,
) -> Result<Option<ChapterDto>, sqlx::Error> {
    // 1) The chapter row joined to its party (guards sigla/org consistency:
    //    a valid id under the WRONG sigla is a miss, not a leak).
    let row: Option<ChapterRow> = sqlx::query_as(
        r"
        SELECT p.sigla, p.name, p.logo_url,
               d.esfera, d.uf, d.municipio, d.name, d.parent_directory_id
          FROM party_directory d
          JOIN party p ON p.org_id = d.org_id AND p.sigla = d.party_sigla
         WHERE d.org_id = $1 AND d.party_sigla = $2 AND d.id = $3
        ",
    )
    .bind(org_id)
    .bind(sigla)
    .bind(id)
    .fetch_optional(db)
    .await?;
    let Some((party_short_name, party_name, logo, esfera, uf, municipio, name, parent_id)) = row
    else {
        return Ok(None);
    };

    // 2) Administrators scoped to THIS chapter. Same privacy filter as the
    //    party detail: accepted only; handle/display_name only.
    let admin_rows: Vec<AdminRow> = sqlx::query_as(
        r"
        SELECT c.handle, c.display_name, pa.role, pa.directory_id
          FROM party_administrator pa
          JOIN citizen c ON c.id = pa.citizen_id
         WHERE pa.org_id = $1 AND pa.party_sigla = $2 AND pa.directory_id = $3
           AND pa.accepted_at IS NOT NULL
         ORDER BY pa.created_at ASC
        ",
    )
    .bind(org_id)
    .bind(&party_short_name)
    .bind(id)
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

    Ok(Some(ChapterDto {
        id,
        party_short_name,
        party_name,
        party_logo_url: logo,
        level: esfera_to_level(&esfera).to_owned(),
        state: uf.map(|u| u.trim().to_uppercase()),
        municipality: municipio,
        name,
        parent_id,
        administrators,
    }))
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
// Write surface (0.37.0 — Fase 2.1)
// ---------------------------------------------------------------------------

/// Corpo de `POST /parties/{sigla}/directories`. A esfera determina quais campos
/// territorial fields are mandatory (the database CHECK enforces it, but we validate
/// early for a friendly 400 instead of a constraint 500).
#[derive(Debug, Deserialize)]
pub struct CreateDirectoryBody {
    pub org_id: Uuid,
    /// 'federal' | 'estadual' | 'municipal'.
    pub esfera: String,
    /// UF (2 letters) — mandatory for state/municipal, forbidden for federal.
    pub uf: Option<String>,
    /// Municipality — mandatory only for municipal.
    pub municipio: Option<String>,
    /// Directory name (e.g. "Diretório Municipal do PT — Porto Alegre").
    pub name: String,
    /// Parent in the tree (municipal→state→federal). Optional.
    pub parent_directory_id: Option<Uuid>,
    /// The citizen responsible for the directory — either this field OR
    /// `responsavel_handle` is mandatory. Born as a `party_administrator` with role
    /// `admin` scoped to the directory, in the same transaction (never an orphan directory).
    pub responsavel_citizen_id: Option<Uuid>,
    /// Responsible citizen by `@handle` (resolved server-side, `@` optional).
    pub responsavel_handle: Option<String>,
}

/// A member derived from a directory: a party mandate in that territory.
/// `avatar_url` already arrives resolved (same pattern as `politicos_ext`/`MandateDto`),
/// not the raw object key — the front end only has to display it.
#[derive(Debug, Clone, Serialize)]
pub struct DirectoryMemberDto {
    pub mandate_id: Uuid,
    pub display_name: String,
    pub office: String,
    pub uf: Option<String>,
    pub municipio: Option<String>,
    pub avatar_url: Option<String>,
}

fn fail(status: StatusCode, code: &str, msg: &str) -> Response {
    (status, Json(ApiResponse::<()>::fail(code, msg))).into_response()
}

/// Resolve the avatar object key to a public URL (MEDIA_BASE_URL). Same
/// behaviour as `gateway::politicos_ext::resolve_avatar`, replicated here
/// because that one is private to the gateway crate.
fn resolve_avatar(object_key: Option<&str>) -> Option<String> {
    let key = object_key?.trim();
    if key.is_empty() {
        return None;
    }
    let base = std::env::var("MEDIA_BASE_URL")
        .unwrap_or_else(|_| "https://democracia.social.br/media".to_owned());
    Some(format!("{}/{}", base.trim_end_matches('/'), key))
}

/// Write gate of the party surface: platform admin/owner OR the party's NATIONAL
/// admin (party_administrator with directory_id NULL, role='admin', accepted).
/// Same criterion as `mandate_invite::invite_authorized`; `moderador` does not
/// qualify — creating a directory reorganizes the party's structure.
async fn party_write_authorized(
    db: &sqlx::PgPool,
    org: Uuid,
    citizen: Uuid,
    sigla: &str,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(
        r"SELECT EXISTS(
             SELECT 1 FROM admin_role_binding
              WHERE org_id = $1 AND citizen_id = $2 AND role IN ('owner','admin')
          ) OR EXISTS(
             SELECT 1 FROM party_administrator
              WHERE org_id = $1 AND citizen_id = $2 AND party_sigla = $3
                AND role = 'admin' AND directory_id IS NULL
                AND (accepted_at IS NOT NULL OR invited_by IS NULL)
          )",
    )
    .bind(org)
    .bind(citizen)
    .bind(sigla)
    .fetch_one(db)
    .await
}

async fn create_directory(
    State(state): State<AppState>,
    caller: CallerId,
    Path(sigla): Path<String>,
    Json(body): Json<CreateDirectoryBody>,
) -> Response {
    // The authenticated caller must act within their own org (CallerId is already
    // resolved from the session; the body's org_id must match so tenants never cross).
    if caller.org.as_uuid() != body.org_id {
        return fail(
            StatusCode::FORBIDDEN,
            "org_mismatch",
            "Organização inválida.",
        );
    }
    // Normalization + federative validation (mirrors the party_directory_esfera_shape CHECK).
    let esfera = body.esfera.trim().to_lowercase();
    if !matches!(esfera.as_str(), "federal" | "estadual" | "municipal") {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_esfera",
            "esfera deve ser federal, estadual ou municipal.",
        );
    }
    let uf = body
        .uf
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_uppercase);
    if let Some(u) = &uf {
        if u.len() != 2 || !u.chars().all(|c| c.is_ascii_alphabetic()) {
            return fail(
                StatusCode::BAD_REQUEST,
                "invalid_uf",
                "UF inválida (2 letras).",
            );
        }
    }
    let municipio = body
        .municipio
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let shape_ok = match esfera.as_str() {
        "federal" => uf.is_none() && municipio.is_none(),
        "estadual" => uf.is_some() && municipio.is_none(),
        "municipal" => uf.is_some() && municipio.is_some(),
        _ => false,
    };
    if !shape_ok {
        return fail(
            StatusCode::BAD_REQUEST,
            "federative_shape",
            "federal não leva UF/município; estadual exige UF; municipal exige UF e município.",
        );
    }
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 160 {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_name",
            "nome do diretório deve ter de 1 a 160 caracteres.",
        );
    }

    // Authorization.
    match party_write_authorized(&state.db, body.org_id, caller.citizen.as_uuid(), &sigla).await {
        Ok(true) => {}
        Ok(false) => {
            return fail(
                StatusCode::FORBIDDEN,
                "not_party_admin",
                "Apenas administradores da plataforma ou do partido podem criar diretórios.",
            )
        }
        Err(err) => {
            tracing::error!(error = ?err, "party directory: authz check");
            return fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage",
                "Erro interno.",
            );
        }
    }

    // Mandatory responsible citizen: every directory is born with someone answering
    // for it (party_administrator role 'admin' scoped to the directory).
    let responsavel = if let Some(id) = body.responsavel_citizen_id {
        id
    } else if let Some(handle) = body
        .responsavel_handle
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let handle = handle.trim_start_matches('@');
        match sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM citizen WHERE handle = $1 AND org_id = $2",
        )
        .bind(handle)
        .bind(body.org_id)
        .fetch_optional(&state.db)
        .await
        {
            Ok(Some(id)) => id,
            Ok(None) => {
                return fail(
                    StatusCode::NOT_FOUND,
                    "responsavel_not_found",
                    "Responsável não encontrado por handle nesta organização.",
                )
            }
            Err(err) => {
                tracing::error!(error = ?err, "party directory: resolve responsável");
                return fail(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "storage",
                    "Erro interno.",
                );
            }
        }
    } else {
        return fail(
            StatusCode::BAD_REQUEST,
            "missing_responsavel",
            "Informe o responsável pelo diretório (responsavel_citizen_id ou responsavel_handle).",
        );
    };

    // Single transaction: directory + the responsible binding, or nothing.
    let created: Result<Uuid, sqlx::Error> = async {
        let mut tx = state.db.begin().await?;
        let id: Uuid = sqlx::query_scalar(
            r"INSERT INTO party_directory
                (org_id, party_sigla, esfera, uf, municipio, name, parent_directory_id)
              VALUES ($1, $2, $3, $4, $5, $6, $7)
              RETURNING id",
        )
        .bind(body.org_id)
        .bind(&sigla)
        .bind(&esfera)
        .bind(uf.as_deref())
        .bind(municipio.as_deref())
        .bind(name)
        .bind(body.parent_directory_id)
        .fetch_one(&mut *tx)
        .await?;
        // `accepted_at = now()`: direct designation by someone who already holds write power
        // no partido, mesmo shape do assign do gateway (admin_parties).
        sqlx::query(
            r"INSERT INTO party_administrator
                (org_id, party_sigla, directory_id, citizen_id, role, invited_by, accepted_at)
              VALUES ($1, $2, $3, $4, 'admin', $5, now())",
        )
        .bind(body.org_id)
        .bind(&sigla)
        .bind(id)
        .bind(responsavel)
        .bind(caller.citizen.as_uuid())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(id)
    }
    .await;

    match created {
        Ok(id) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(serde_json::json!({ "id": id }))),
        )
            .into_response(),
        // Territorial index (0673): a directory of this party already exists in this
        // territory — a double click/retry must not create another.
        Err(sqlx::Error::Database(dberr)) if dberr.is_unique_violation() => fail(
            StatusCode::CONFLICT,
            "directory_exists",
            "Já existe um diretório deste partido neste território.",
        ),
        // FK failure: sigla absent in the org, invalid parent, or the responsible
        // citizen (by direct id) does not exist → friendly 404.
        Err(sqlx::Error::Database(dberr)) if dberr.is_foreign_key_violation() => {
            if dberr.constraint().is_some_and(|c| c.contains("citizen")) {
                fail(
                    StatusCode::NOT_FOUND,
                    "responsavel_not_found",
                    "Responsável não encontrado nesta organização.",
                )
            } else {
                fail(
                    StatusCode::NOT_FOUND,
                    "party_or_parent_not_found",
                    "Partido não encontrado nesta organização, ou diretório-pai inválido.",
                )
            }
        }
        Err(err) => {
            tracing::error!(error = ?err, sigla, "party directory: insert");
            fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage",
                "Erro interno.",
            )
        }
    }
}

async fn delete_directory(
    State(state): State<AppState>,
    caller: CallerId,
    Path((sigla, id)): Path<(String, Uuid)>,
    Query(query): Query<OrgQuery>,
) -> Response {
    if caller.org.as_uuid() != query.org_id {
        return fail(
            StatusCode::FORBIDDEN,
            "org_mismatch",
            "Organização inválida.",
        );
    }
    match party_write_authorized(&state.db, query.org_id, caller.citizen.as_uuid(), &sigla).await {
        Ok(true) => {}
        Ok(false) => {
            return fail(
                StatusCode::FORBIDDEN,
                "not_party_admin",
                "Apenas administradores da plataforma ou do partido podem remover diretórios.",
            )
        }
        Err(err) => {
            tracing::error!(error = ?err, "party directory: authz check");
            return fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage",
                "Erro interno.",
            );
        }
    }
    // Do not allow removing a directory that still parents another (the tree would be orphaned).
    let has_children: Result<bool, sqlx::Error> = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM party_directory WHERE parent_directory_id = $1)",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await;
    if matches!(has_children, Ok(true)) {
        return fail(
            StatusCode::CONFLICT,
            "has_children",
            "Remova os diretórios-filhos antes deste.",
        );
    }
    let res = sqlx::query(
        "DELETE FROM party_directory WHERE id = $1 AND org_id = $2 AND party_sigla = $3",
    )
    .bind(id)
    .bind(query.org_id)
    .bind(&sigla)
    .execute(&state.db)
    .await;
    match res {
        Ok(r) if r.rows_affected() == 0 => fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "Diretório não encontrado.",
        ),
        Ok(_) => (StatusCode::OK, Json(ApiResponse::ok(()))).into_response(),
        Err(err) => {
            tracing::error!(error = ?err, "party directory: delete");
            fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage",
                "Erro interno.",
            )
        }
    }
}

async fn list_directory_members(
    State(state): State<AppState>,
    Path((sigla, id)): Path<(String, Uuid)>,
    Query(query): Query<OrgQuery>,
) -> Json<ApiResponse<Vec<DirectoryMemberDto>>> {
    match load_directory_members(&state.db, query.org_id, &sigla, id).await {
        Ok(list) => Json(ApiResponse::ok(list)),
        Err(err) => {
            tracing::error!(error = ?err, sigla, "party directory: members");
            Json(ApiResponse::ok(Vec::new()))
        }
    }
}

// ---------------------------------------------------------------------------
// SQL (runtime queries — no `.sqlx/` cache regeneration needed)
// ---------------------------------------------------------------------------

/// (sigla, name, tse_number, logo_url, website, founded_year, mandate_count, fed, est, mun)
type PartyRow = (
    String,
    String,
    Option<i32>,
    Option<String>,
    Option<String>,
    Option<i32>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
);
/// (id, party_sigla, esfera, uf, municipio, name, parent_directory_id)
type DirectoryRow = (
    Uuid,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
    Option<Uuid>,
);
/// (handle, display_name, role, directory_id)
type AdminRow = (Option<String>, Option<String>, String, Option<Uuid>);
/// (mandate_id, display_name, office, uf, municipio, avatar_object_key)
type MemberRow = (
    Uuid,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

async fn load_parties(db: &sqlx::PgPool, org_id: Uuid) -> Result<Vec<PartyDto>, sqlx::Error> {
    // LEFT JOIN so a freshly-created party with zero mandates still appears (count = 0).
    // ORDER BY mandate_count DESC, then sigla for a stable tie-break.
    let rows: Vec<PartyRow> = sqlx::query_as(
        r"
            SELECT p.sigla,
                   p.name,
                   p.tse_number,
                   p.logo_url,
                   p.website,
                   p.founded_year,
                   COUNT(m.id) AS mandate_count,
                   COUNT(m.id) FILTER (WHERE m.sphere = 'federal')   AS federal,
                   COUNT(m.id) FILTER (WHERE m.sphere = 'estadual')  AS estadual,
                   COUNT(m.id) FILTER (WHERE m.sphere = 'municipal') AS municipal
              FROM party p
              LEFT JOIN mandate m
                     ON m.org_id = p.org_id
                    AND m.party  = p.sigla
                    AND m.hidden_at IS NULL
             WHERE p.org_id = $1
             GROUP BY p.sigla, p.name, p.tse_number, p.logo_url, p.website, p.founded_year
             ORDER BY COUNT(m.id) DESC, p.sigla ASC
            ",
    )
    .bind(org_id)
    .fetch_all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(sigla, name, tse, logo, website, year, count, fed, est, mun)| PartyDto {
                sigla,
                name,
                tse_number: tse,
                logo_url: logo,
                website,
                founded_year: year,
                mandate_count: count.unwrap_or(0),
                federal_count: fed.unwrap_or(0),
                estadual_count: est.unwrap_or(0),
                municipal_count: mun.unwrap_or(0),
            },
        )
        .collect())
}

async fn load_party_detail(
    db: &sqlx::PgPool,
    org_id: Uuid,
    sigla: &str,
) -> Result<Option<PartyDetailDto>, sqlx::Error> {
    // 1) The party row + derived mandate count.
    let party_row: Option<PartyRow> = sqlx::query_as(
        r"
            SELECT p.sigla,
                   p.name,
                   p.tse_number,
                   p.logo_url,
                   p.website,
                   p.founded_year,
                   COUNT(m.id) AS mandate_count,
                   COUNT(m.id) FILTER (WHERE m.sphere = 'federal')   AS federal,
                   COUNT(m.id) FILTER (WHERE m.sphere = 'estadual')  AS estadual,
                   COUNT(m.id) FILTER (WHERE m.sphere = 'municipal') AS municipal
              FROM party p
              LEFT JOIN mandate m
                     ON m.org_id = p.org_id
                    AND m.party  = p.sigla
                    AND m.hidden_at IS NULL
             WHERE p.org_id = $1 AND p.sigla = $2
             GROUP BY p.sigla, p.name, p.tse_number, p.logo_url, p.website, p.founded_year
            ",
    )
    .bind(org_id)
    .bind(sigla)
    .fetch_optional(db)
    .await?;

    let Some((s, name, tse, logo, website, year, count, fed, est, mun)) = party_row else {
        return Ok(None);
    };
    let party = PartyDto {
        sigla: s,
        name,
        tse_number: tse,
        logo_url: logo,
        website,
        founded_year: year,
        mandate_count: count.unwrap_or(0),
        federal_count: fed.unwrap_or(0),
        estadual_count: est.unwrap_or(0),
        municipal_count: mun.unwrap_or(0),
    };

    // 2) Directories (any esfera). Ordered federal → estadual → municipal by natural sort.
    let dir_rows: Vec<DirectoryRow> = sqlx::query_as(
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
        .map(
            |(id, party_sigla, esfera, uf, municipio, name, parent)| PartyDirectoryDto {
                id,
                party_sigla,
                esfera,
                uf,
                municipio,
                name,
                parent_directory_id: parent,
            },
        )
        .collect();

    // 3) Administrators. Privacy filter: only accepted admins are exposed publicly.
    // Only expose (handle, display_name, role, directory scope). Never the citizen id.
    let admin_rows: Vec<AdminRow> = sqlx::query_as(
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

/// Members derived from a directory: the mandates of the same party whose sphere and
/// territory match the directory. Municipal matches on (party, sphere, uf,
/// municipio); estadual por (party, sphere, uf); federal por (party, sphere).
/// It is the same derivation `PartyDetail.svelte` did client-side — now on the
/// server, anchored to a real directory.
async fn load_directory_members(
    db: &sqlx::PgPool,
    org_id: Uuid,
    sigla: &str,
    directory_id: Uuid,
) -> Result<Vec<DirectoryMemberDto>, sqlx::Error> {
    // 1) Resolve the directory's territory (and prove it belongs to this org + party).
    let dir: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
        r"SELECT esfera, uf, municipio
            FROM party_directory
           WHERE id = $1 AND org_id = $2 AND party_sigla = $3",
    )
    .bind(directory_id)
    .bind(org_id)
    .bind(sigla)
    .fetch_optional(db)
    .await?;
    let Some((esfera, uf, municipio)) = dir else {
        return Ok(Vec::new());
    };

    // 2) The party's mandates in the territory. The uf/municipio filters only apply
    // when the sphere defines them (federal ignores both; state ignores
    // municipality) — implemented with the `$4::text IS NULL OR col = $4` guard.
    let rows: Vec<MemberRow> = sqlx::query_as(
        r"
            SELECT id, display_name, office, uf, municipio, avatar_object_key
              FROM mandate
             WHERE org_id = $1
               AND hidden_at IS NULL
               AND party = $2
               AND sphere = $3
               AND ($4::text IS NULL OR uf = $4)
               AND ($5::text IS NULL OR municipio = $5)
             ORDER BY display_name ASC
            ",
    )
    .bind(org_id)
    .bind(sigla)
    .bind(&esfera)
    .bind(uf.as_deref())
    .bind(municipio.as_deref())
    .fetch_all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(mandate_id, display_name, office, uf, municipio, avatar)| DirectoryMemberDto {
                mandate_id,
                display_name,
                office,
                uf,
                municipio,
                avatar_url: resolve_avatar(avatar.as_deref()),
            },
        )
        .collect())
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
            website: None,
            founded_year: Some(1980),
            mandate_count: 42,
            federal_count: 10,
            estadual_count: 20,
            municipal_count: 12,
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
                website: None,
                founded_year: None,
                mandate_count: 3,
                federal_count: 3,
                estadual_count: 0,
                municipal_count: 0,
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

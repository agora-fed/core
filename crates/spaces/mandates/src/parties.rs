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
    /// Mandates currently attributed to this sigla in the org (derived, não-ocultos).
    pub mandate_count: i64,
    /// Contagem por esfera — evita a página baixar todos os mandatos pra derivar.
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
        // Write surface (0.37.0 — Fase 2.1): criar/remover diretórios subnacionais.
        // Gate: admin de plataforma OU admin nacional do partido (party_write_authorized).
        .route("/parties/{sigla}/directories", post(create_directory))
        .route(
            "/parties/{sigla}/directories/{id}",
            axum::routing::delete(delete_directory),
        )
        // Membros derivados do diretório: os mandatos do partido naquele território.
        // Público (read-only) — mesma lógica territorial que a PartyDetail derivava no client.
        .route(
            "/parties/{sigla}/directories/{id}/members",
            get(list_directory_members),
        )
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
// Write surface (0.37.0 — Fase 2.1)
// ---------------------------------------------------------------------------

/// Corpo de `POST /parties/{sigla}/directories`. A esfera determina quais campos
/// territoriais são obrigatórios (o CHECK do banco reforça, mas validamos cedo
/// pra um 400 amigável em vez de um 500 de constraint).
#[derive(Debug, Deserialize)]
pub struct CreateDirectoryBody {
    pub org_id: Uuid,
    /// 'federal' | 'estadual' | 'municipal'.
    pub esfera: String,
    /// UF (2 letras) — obrigatória em estadual/municipal, proibida em federal.
    pub uf: Option<String>,
    /// Município — obrigatório só em municipal.
    pub municipio: Option<String>,
    /// Nome do diretório (ex.: "Diretório Municipal do PT — Porto Alegre").
    pub name: String,
    /// Pai na árvore (municipal→estadual→federal). Opcional.
    pub parent_directory_id: Option<Uuid>,
}

/// Membro derivado de um diretório: um mandato do partido naquele território.
/// `avatar_url` já vem resolvido (mesmo padrão de `politicos_ext`/`MandateDto`),
/// não a object key crua — o front só precisa exibir.
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

/// Resolve a object key do avatar para URL pública (MEDIA_BASE_URL). Mesmo
/// comportamento de `gateway::politicos_ext::resolve_avatar`, replicado aqui
/// porque aquele é privado ao crate do gateway.
fn resolve_avatar(object_key: Option<&str>) -> Option<String> {
    let key = object_key?.trim();
    if key.is_empty() {
        return None;
    }
    let base = std::env::var("MEDIA_BASE_URL")
        .unwrap_or_else(|_| "https://democracia.social.br/media".to_owned());
    Some(format!("{}/{}", base.trim_end_matches('/'), key))
}

/// Gate de escrita da superfície de partido: admin/owner de plataforma OU admin
/// NACIONAL do partido (party_administrator com directory_id NULL, role='admin',
/// aceito). Mesmo critério do `mandate_invite::invite_authorized`; `moderador`
/// não qualifica — criar diretório reorganiza a estrutura do partido.
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
    // O caller autenticado precisa agir na sua própria org (o CallerId já é
    // resolvido da sessão; o org_id do corpo tem que bater pra não cruzar tenant).
    if caller.org.as_uuid() != body.org_id {
        return fail(
            StatusCode::FORBIDDEN,
            "org_mismatch",
            "Organização inválida.",
        );
    }
    // Normalização + validação federativa (espelha o CHECK party_directory_esfera_shape).
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

    // Autorização.
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

    let id: Result<Uuid, sqlx::Error> = sqlx::query_scalar(
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
    .fetch_one(&state.db)
    .await;

    match id {
        Ok(id) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(serde_json::json!({ "id": id }))),
        )
            .into_response(),
        // FK falha (sigla inexistente na org, ou parent inválido) → 404/409 amigável.
        Err(sqlx::Error::Database(dberr)) if dberr.is_foreign_key_violation() => fail(
            StatusCode::NOT_FOUND,
            "party_or_parent_not_found",
            "Partido não encontrado nesta organização, ou diretório-pai inválido.",
        ),
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
    // Não deixa remover um diretório que ainda é pai de outro (a árvore ficaria órfã).
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

/// Membros derivados de um diretório: os mandatos do mesmo partido cuja esfera e
/// território batem com o diretório. Municipal casa por (party, sphere, uf,
/// municipio); estadual por (party, sphere, uf); federal por (party, sphere).
/// É a mesma derivação que a `PartyDetail.svelte` fazia no client — agora no
/// servidor, ancorada num diretório real.
async fn load_directory_members(
    db: &sqlx::PgPool,
    org_id: Uuid,
    sigla: &str,
    directory_id: Uuid,
) -> Result<Vec<DirectoryMemberDto>, sqlx::Error> {
    // 1) Resolve o território do diretório (e prova que pertence a esta org + partido).
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

    // 2) Mandatos do partido no território. Os filtros de uf/municipio só se
    // aplicam quando a esfera os define (federal ignora ambos; estadual ignora
    // municipio) — implementado com o guard `$4::text IS NULL OR col = $4`.
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

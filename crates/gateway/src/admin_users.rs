//! Admin: gerenciamento completo de usuários (0.25.0-fediverso).
//!
//! Endpoints ricos pra o painel `/admin/usuarios`:
//! - `GET /admin/users` — lista com joins (auth_credential, admin_role_binding,
//!   party_administrator, mandate_identity_binding, candidacy). Filtros:
//!   `q` (handle/email/display_name), `party` (party_sigla), `platform_role`
//!   (owner|admin|auditor|none), `party_role` (admin|moderador|none),
//!   `civic_type` (cidadao|politico|candidato|any).
//! - `PATCH /admin/users/{id}` — atualiza campos "de cadastro" (party_sigla,
//!   verification_level, is_public).
//! - `PUT /admin/users/{id}/platform-role` — define ou remove
//!   admin_role_binding (owner/admin/auditor|none).
//! - `PUT /admin/users/{id}/party-role` — define ou remove
//!   party_administrator (admin|moderador|none) + party_sigla.
//!
//! Auth: require_admin (mesmo pattern do email_templates.rs).

use axum::extract::{Json, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, put};
use axum::Router;
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        // GET com filtros ricos — path distinto de `/admin/users` (que é o
        // list "legacy" mais simples, mantido em `admin_ext.rs`).
        .route("/admin/users-rich", get(list))
        .route("/admin/users/{id}", patch(update))
        .route("/admin/users/{id}/platform-role", put(set_platform_role))
        .route("/admin/users/{id}/party-role", put(set_party_role))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Guard
// ---------------------------------------------------------------------------

async fn require_admin(headers: &HeaderMap, db: &PgPool) -> std::result::Result<Uuid, Response> {
    let citizen_id: Uuid = headers
        .get("x-dsoc-citizen-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .ok_or_else(unauthorized_resp)?;
    let is_admin = sqlx::query_scalar::<_, bool>(
        r"SELECT EXISTS (
             SELECT 1 FROM admin_role_binding
              WHERE citizen_id = $1 AND role IN ('owner','admin')
           )",
    )
    .bind(citizen_id)
    .fetch_one(db)
    .await
    .unwrap_or(false);
    if !is_admin {
        return Err(forbidden_resp());
    }
    Ok(citizen_id)
}

fn unauthorized_resp() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(ApiResponse::<()>::fail(
            "unauthorized",
            "Autenticação necessária.",
        )),
    )
        .into_response()
}
fn forbidden_resp() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(ApiResponse::<()>::fail(
            "forbidden",
            "Acesso restrito a admins.",
        )),
    )
        .into_response()
}
fn storage_resp(err: impl std::fmt::Debug) -> Response {
    tracing::error!(?err, "admin_users storage error");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiResponse::<()>::fail("storage_error", "Erro interno.")),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Row
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct AdminUserRow {
    pub citizen_id: Uuid,
    pub display_name: Option<String>,
    pub handle: Option<String>,
    pub email: Option<String>,
    pub verification_level: String,
    pub is_public: bool,
    pub titulo_status: Option<String>,
    pub party_sigla: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    // Papel na plataforma (owner|admin|auditor) — NULL se não tem.
    pub platform_role: Option<String>,
    // Papel de partido — (party_sigla, role). Se admin de mais de um, pega o primeiro.
    pub party_admin_sigla: Option<String>,
    pub party_admin_role: Option<String>,
    // Perfil cívico — flags derivadas.
    pub has_mandate: bool,
    pub has_candidacy: bool,
    // Estado de moderação (0.26.11).
    pub suspended_at: Option<chrono::DateTime<chrono::Utc>>,
    pub silenced_at: Option<chrono::DateTime<chrono::Utc>>,
}

// ---------------------------------------------------------------------------
// List
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ListParams {
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    party: Option<String>,
    /// `owner`|`admin`|`auditor`|`none`|`any` (default `any`).
    #[serde(default)]
    platform_role: Option<String>,
    /// `admin`|`moderador`|`none`|`any` (default `any`).
    #[serde(default)]
    party_role: Option<String>,
    /// `cidadao` (sem mandato/candidatura) | `politico` (tem mandate binding)
    /// | `candidato` (tem candidacy) | `any` (default).
    #[serde(default)]
    civic_type: Option<String>,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}
fn default_limit() -> i64 {
    50
}

async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<ListParams>,
) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    let limit = params.limit.clamp(1, 200);
    let offset = params.offset.max(0);
    let q_like = params
        .q
        .as_deref()
        .map(|s| format!("%{}%", s.to_lowercase()));
    let party = params.party.clone();
    let platform_role = params.platform_role.clone();
    let party_role = params.party_role.clone();
    let civic_type = params.civic_type.clone();

    // Query rica com joins. Todas as filtragens são NULL-safe via COALESCE.
    let rows: Vec<AdminUserRow> = match sqlx::query_as(
        r"
        WITH
          -- Um cidadão pode ter mais de um role no binding (ex.: admin +
          -- owner). Colapsa pro mais alto: owner > admin > auditor.
          plat AS (
            SELECT citizen_id,
                   CASE
                     WHEN bool_or(role = 'owner')   THEN 'owner'
                     WHEN bool_or(role = 'admin')   THEN 'admin'
                     WHEN bool_or(role = 'auditor') THEN 'auditor'
                   END AS role
              FROM admin_role_binding
             GROUP BY citizen_id
          ),
          party_admin AS (
            SELECT citizen_id,
                   MIN(party_sigla) AS party_sigla,
                   MIN(role)        AS role
              FROM party_administrator
             GROUP BY citizen_id
          ),
          mib_agg AS (
            SELECT citizen_id, TRUE AS has_it
              FROM mandate_identity_binding
             GROUP BY citizen_id
          ),
          cand AS (
            SELECT c.mandate_id
              FROM candidacy c
             GROUP BY c.mandate_id
          )
        SELECT
          c.id                  AS citizen_id,
          c.display_name,
          c.handle,
          ac.email              AS email,
          c.verification_level,
          c.is_public,
          c.titulo_status,
          c.party_sigla,
          c.created_at,
          plat.role             AS platform_role,
          party_admin.party_sigla AS party_admin_sigla,
          party_admin.role      AS party_admin_role,
          COALESCE(mib_agg.has_it, FALSE) AS has_mandate,
          -- Cidadão como candidato: se tem binding em mandate is_candidate
          -- OU se o mandato dele aparece em candidacy.
          COALESCE(
            (SELECT bool_or(m.is_candidate)
               FROM mandate_identity_binding mib
               JOIN mandate m ON m.id = mib.mandate_id
              WHERE mib.citizen_id = c.id),
            FALSE
          )                     AS has_candidacy,
          c.suspended_at,
          c.silenced_at
          FROM citizen c
          LEFT JOIN auth_credential ac ON ac.citizen_id = c.id
          LEFT JOIN plat         ON plat.citizen_id = c.id
          LEFT JOIN party_admin  ON party_admin.citizen_id = c.id
          LEFT JOIN mib_agg      ON mib_agg.citizen_id = c.id
         WHERE
          -- q (handle/email/display_name)
          ($1::text IS NULL
           OR lower(COALESCE(c.handle, ''))       LIKE $1
           OR lower(COALESCE(c.display_name, '')) LIKE $1
           OR lower(COALESCE(ac.email, ''))       LIKE $1)
          -- party (filiação)
          AND ($2::text IS NULL OR c.party_sigla = $2)
          -- platform_role
          AND (
            $3::text IS NULL OR $3 = 'any'
            OR ($3 = 'none' AND plat.role IS NULL)
            OR plat.role = $3
          )
          -- party_role
          AND (
            $4::text IS NULL OR $4 = 'any'
            OR ($4 = 'none' AND party_admin.role IS NULL)
            OR party_admin.role = $4
          )
          -- civic_type
          AND (
            $5::text IS NULL OR $5 = 'any'
            OR ($5 = 'politico' AND COALESCE(mib_agg.has_it, FALSE) = TRUE)
            OR ($5 = 'candidato' AND EXISTS (
                 SELECT 1 FROM mandate_identity_binding mib
                   JOIN mandate m ON m.id = mib.mandate_id
                  WHERE mib.citizen_id = c.id AND m.is_candidate = TRUE
               ))
            OR ($5 = 'cidadao' AND NOT EXISTS (
                 SELECT 1 FROM mandate_identity_binding mib
                  WHERE mib.citizen_id = c.id
               ))
          )
         ORDER BY c.created_at DESC
         LIMIT $6 OFFSET $7
        ",
    )
    .bind(q_like)
    .bind(party)
    .bind(platform_role)
    .bind(party_role)
    .bind(civic_type)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    {
        Ok(r) => r,
        Err(err) => return storage_resp(err),
    };

    (StatusCode::OK, Json(ApiResponse::ok(rows))).into_response()
}

// ---------------------------------------------------------------------------
// PATCH citizen fields
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct UpdateBody {
    /// `Some("")` limpa (SET NULL), `Some(v)` altera, `None` deixa.
    #[serde(default)]
    party_sigla: Option<Option<String>>,
    #[serde(default)]
    verification_level: Option<String>,
    #[serde(default)]
    is_public: Option<bool>,
}

async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(citizen_id): Path<Uuid>,
    Json(body): Json<UpdateBody>,
) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    // Normaliza party_sigla: "" → NULL.
    let party_arg: Option<Option<String>> = body.party_sigla.map(|inner| {
        inner
            .and_then(|s| if s.trim().is_empty() { None } else { Some(s) })
            .or(None)
    });
    // party_arg: Some(Some("PT")) atualiza, Some(None) seta NULL, None ignora.
    let (party_set, party_value) = match party_arg {
        Some(Some(s)) => (true, Some(s)),
        Some(None) => (true, None),
        None => (false, None),
    };
    let (verif_set, verif_value) = match body.verification_level.as_deref() {
        Some(v) if !v.is_empty() => (true, Some(v.to_owned())),
        _ => (false, None),
    };
    let (public_set, public_value) = match body.is_public {
        Some(v) => (true, v),
        None => (false, false),
    };

    if let Err(err) = sqlx::query(
        r"UPDATE citizen
             SET party_sigla        = CASE WHEN $2 THEN $3 ELSE party_sigla END,
                 verification_level = CASE WHEN $4 THEN $5 ELSE verification_level END,
                 is_public          = CASE WHEN $6 THEN $7 ELSE is_public END,
                 profile_updated_at = now()
           WHERE id = $1",
    )
    .bind(citizen_id)
    .bind(party_set)
    .bind(party_value)
    .bind(verif_set)
    .bind(verif_value)
    .bind(public_set)
    .bind(public_value)
    .execute(&state.db)
    .await
    {
        return storage_resp(err);
    }
    (StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response()
}

// ---------------------------------------------------------------------------
// Platform role
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct PlatformRoleBody {
    /// `owner`|`admin`|`auditor`|`none` (remove).
    role: String,
    /// Org sob a qual o papel vale. Se ausente, usamos o primeiro org da
    /// tabela (single-org install é o caso mais comum hoje).
    #[serde(default)]
    org_id: Option<Uuid>,
}

async fn set_platform_role(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(citizen_id): Path<Uuid>,
    Json(body): Json<PlatformRoleBody>,
) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    // Org resolvido do body ou da citizen row.
    let org_id = match body.org_id {
        Some(o) => o,
        None => {
            match sqlx::query_scalar::<_, Uuid>("SELECT org_id FROM citizen WHERE id = $1")
                .bind(citizen_id)
                .fetch_optional(&state.db)
                .await
            {
                Ok(Some(o)) => o,
                _ => {
                    return (
                        StatusCode::NOT_FOUND,
                        Json(ApiResponse::<()>::fail(
                            "not_found",
                            "Cidadão não encontrado.",
                        )),
                    )
                        .into_response();
                }
            }
        }
    };
    if body.role == "none" {
        if let Err(err) =
            sqlx::query("DELETE FROM admin_role_binding WHERE citizen_id = $1 AND org_id = $2")
                .bind(citizen_id)
                .bind(org_id)
                .execute(&state.db)
                .await
        {
            return storage_resp(err);
        }
    } else if matches!(body.role.as_str(), "owner" | "admin" | "auditor") {
        // Substitui qualquer role antigo por este novo. Duas queries em
        // uma tx — sqlx prepared statement não aceita `DELETE ...; INSERT`.
        let mut tx = match state.db.begin().await {
            Ok(t) => t,
            Err(err) => return storage_resp(err),
        };
        if let Err(err) =
            sqlx::query(r"DELETE FROM admin_role_binding WHERE citizen_id = $1 AND org_id = $2")
                .bind(citizen_id)
                .bind(org_id)
                .execute(&mut *tx)
                .await
        {
            return storage_resp(err);
        }
        if let Err(err) = sqlx::query(
            r"INSERT INTO admin_role_binding (id, org_id, citizen_id, role, created_at)
              VALUES ($1, $2, $3, $4, now())",
        )
        .bind(Uuid::now_v7())
        .bind(org_id)
        .bind(citizen_id)
        .bind(&body.role)
        .execute(&mut *tx)
        .await
        {
            return storage_resp(err);
        }
        if let Err(err) = tx.commit().await {
            return storage_resp(err);
        }
    } else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::fail("invalid_input", "Role inválido.")),
        )
            .into_response();
    }
    (StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response()
}

// ---------------------------------------------------------------------------
// Party role
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct PartyRoleBody {
    /// `admin`|`moderador`|`none` (remove).
    role: String,
    /// Obrigatório quando role ≠ 'none'.
    #[serde(default)]
    party_sigla: Option<String>,
    #[serde(default)]
    org_id: Option<Uuid>,
}

async fn set_party_role(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(citizen_id): Path<Uuid>,
    Json(body): Json<PartyRoleBody>,
) -> Response {
    if let Err(r) = require_admin(&headers, &state.db).await {
        return r;
    }
    let org_id = match body.org_id {
        Some(o) => o,
        None => match sqlx::query_scalar::<_, Uuid>("SELECT org_id FROM citizen WHERE id = $1")
            .bind(citizen_id)
            .fetch_optional(&state.db)
            .await
        {
            Ok(Some(o)) => o,
            _ => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(ApiResponse::<()>::fail(
                        "not_found",
                        "Cidadão não encontrado.",
                    )),
                )
                    .into_response();
            }
        },
    };
    if body.role == "none" {
        if let Err(err) =
            sqlx::query("DELETE FROM party_administrator WHERE citizen_id = $1 AND org_id = $2")
                .bind(citizen_id)
                .bind(org_id)
                .execute(&state.db)
                .await
        {
            return storage_resp(err);
        }
    } else if matches!(body.role.as_str(), "admin" | "moderador") {
        let party = match body.party_sigla.as_deref() {
            Some(p) if !p.is_empty() => p,
            _ => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ApiResponse::<()>::fail(
                        "invalid_input",
                        "party_sigla obrigatório quando role != 'none'.",
                    )),
                )
                    .into_response();
            }
        };
        // Substitui qualquer papel antigo do cidadão nessa org pra evitar
        // duplicidade (poderíamos permitir vários partidos por cidadão, mas
        // a UI trata como 1:1). Duas queries em uma tx — sqlx prepared
        // statement não aceita `DELETE ...; INSERT`.
        let mut tx = match state.db.begin().await {
            Ok(t) => t,
            Err(err) => return storage_resp(err),
        };
        if let Err(err) =
            sqlx::query(r"DELETE FROM party_administrator WHERE citizen_id = $1 AND org_id = $2")
                .bind(citizen_id)
                .bind(org_id)
                .execute(&mut *tx)
                .await
        {
            return storage_resp(err);
        }
        if let Err(err) = sqlx::query(
            r"INSERT INTO party_administrator (org_id, party_sigla, citizen_id, role, created_at)
              VALUES ($1, $2, $3, $4, now())",
        )
        .bind(org_id)
        .bind(party)
        .bind(citizen_id)
        .bind(&body.role)
        .execute(&mut *tx)
        .await
        {
            return storage_resp(err);
        }
        if let Err(err) = tx.commit().await {
            return storage_resp(err);
        }
    } else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::fail("invalid_input", "Role inválido.")),
        )
            .into_response();
    }
    (StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response()
}

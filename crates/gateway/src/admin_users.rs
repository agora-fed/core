//! Admin: complete user management (0.25.0-fediverse).
//!
//! Rich endpoints for the `/admin/usuarios` panel:
//! - `GET /admin/users` — a list with joins (auth_credential, admin_role_binding,
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
//! Auth: require_admin (same pattern as email_templates.rs).

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
        // GET with rich filters — a path distinct from `/admin/users` (which is the
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

/// Org-scoped admin gate — delegates to the single implementation in
/// [`crate::authz_ext::require_org_admin`] (issue #8). This module used to carry
/// its own copy that omitted `org_id`, so an owner of ANY org passed it.
async fn require_admin(headers: &HeaderMap, db: &PgPool) -> Result<Uuid, Response> {
    crate::authz_ext::require_org_admin(db, headers)
        .await
        .map(|a| a.citizen)
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
    // Identity-document verification status (auth_credential.cpf_status): 'validated' | other | NULL.
    pub cpf_status: Option<String>,
    // Personal data (mandatory at signup, 0664). The document arrives MASKED from the server.
    pub legal_name: Option<String>,
    pub gender: Option<String>,
    pub birth_date: Option<chrono::NaiveDate>,
    pub uf: Option<String>,
    pub municipio: Option<String>,
    pub cpf_masked: Option<String>,
    pub party_sigla: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    // Platform role (owner|admin|auditor) — NULL when there is none.
    pub platform_role: Option<String>,
    // Party role — (party_sigla, role). If admin of more than one, take the first.
    pub party_admin_sigla: Option<String>,
    pub party_admin_role: Option<String>,
    // Civic profile — derived flags.
    pub has_mandate: bool,
    pub has_candidacy: bool,
    // Moderation state (0.26.11).
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
    /// `cidadao` (no mandate/candidacy) | `politico` (has a mandate binding)
    /// | `candidato` (has a candidacy) | `any` (default).
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
    // The roles column is scoped to the admin's own org (issue #8) — it used to
    // aggregate every org's bindings, so the list showed strangers as owners.
    let admin_org = match crate::authz_ext::require_org_admin(&state.db, &headers).await {
        Ok(a) => a.org,
        Err(r) => return r,
    };
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

    // A rich query with joins. Every filter is NULL-safe via COALESCE.
    let rows: Vec<AdminUserRow> = match sqlx::query_as(
        r"
        WITH
              -- A citizen may hold more than one role in the binding (e.g. admin +
              -- owner). Collapse to the highest: owner > admin > auditor.
          plat AS (
            SELECT citizen_id,
                   CASE
                     WHEN bool_or(role = 'owner')   THEN 'owner'
                     WHEN bool_or(role = 'admin')   THEN 'admin'
                     WHEN bool_or(role = 'auditor') THEN 'auditor'
                   END AS role
              FROM admin_role_binding
             WHERE org_id = $8
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
          ac.cpf_status,
          c.legal_name,
          c.gender,
          c.birth_date,
          c.uf,
          mi.nome AS municipio,
          CASE WHEN ac.cpf IS NULL OR length(ac.cpf) < 5 THEN NULL
               ELSE left(ac.cpf, 3) || '.***.***-' || right(ac.cpf, 2) END AS cpf_masked,
          c.party_sigla,
          c.created_at,
          plat.role             AS platform_role,
          party_admin.party_sigla AS party_admin_sigla,
          party_admin.role      AS party_admin_role,
          COALESCE(mib_agg.has_it, FALSE) AS has_mandate,
              -- Citizen as a candidate: either they have a binding on a mandate with
              -- is_candidate, or their mandate appears in candidacy.
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
          LEFT JOIN municipio_ibge mi ON mi.codigo_ibge = c.municipio_ibge
          LEFT JOIN plat         ON plat.citizen_id = c.id
          LEFT JOIN party_admin  ON party_admin.citizen_id = c.id
          LEFT JOIN mib_agg      ON mib_agg.citizen_id = c.id
         WHERE
          -- q (handle/email/display_name)
          ($1::text IS NULL
           OR lower(COALESCE(c.handle, ''))       LIKE $1
           OR lower(COALESCE(c.display_name, '')) LIKE $1
           OR lower(COALESCE(ac.email, ''))       LIKE $1)
              -- party (affiliation)
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
    .bind(admin_org)
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
    ///
    /// There is deliberately NO `org_id` here (issue #8). The target org comes from
    /// the authenticated caller; accepting one from the body let an admin of org A
    /// grant themselves `owner` of org B, which is the whole cross-tenant escalation.
    role: String,
}

// ---------------------------------------------------------------------------
// E-mail notice when a role is assigned (0.50.0) — templates approved in review.
// ---------------------------------------------------------------------------

/// Which role was assigned — carries the data the template needs.
enum RoleNotice {
    /// Party admin (sigla).
    PartyAdmin(String),
    /// Party moderator (sigla).
    PartyModerador(String),
    /// Platform role (`owner`|`admin`|`auditor`).
    Platform(String),
}

/// Fires the assignment e-mail in the background (fire-and-forget). The text comes
/// from the editable catalog (`email_template`, keys `role_party_admin` /
/// `role_party_moderador` / `role_platform`) rendered with `{{vars}}`, sent as
/// multipart (text + branded HTML). Best-effort: the role is already stored; if
/// SMTP is absent, the template is missing or the send fails, it only logs — it never
/// fails the admin operation.
fn notify_role_bg(db: &PgPool, citizen_id: Uuid, notice: RoleNotice) {
    let db = db.clone();
    tokio::spawn(async move {
        let Some(cfg) = crate::proposal_delivery::smtp_from_env() else {
            tracing::info!("SMTP ausente; e-mail de designação de papel não enviado");
            return;
        };
        let email: Option<String> =
            sqlx::query_scalar("SELECT email FROM auth_credential WHERE citizen_id = $1")
                .bind(citizen_id)
                .fetch_optional(&db)
                .await
                .ok()
                .flatten();
        let Some(email) = email else {
            return; // sem credencial/e-mail: nada a enviar
        };

        let mut vars: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
        let key = match &notice {
            RoleNotice::PartyAdmin(sigla) => {
                vars.insert("party", sigla.clone());
                vars.insert(
                    "party_url",
                    format!(
                        "https://democracia.social.br/partidos/{}",
                        sigla.to_lowercase()
                    ),
                );
                "role_party_admin"
            }
            RoleNotice::PartyModerador(sigla) => {
                vars.insert("party", sigla.clone());
                vars.insert(
                    "party_url",
                    format!(
                        "https://democracia.social.br/partidos/{}",
                        sigla.to_lowercase()
                    ),
                );
                "role_party_moderador"
            }
            RoleNotice::Platform(role) => {
                let label = match role.as_str() {
                    "owner" => "proprietário(a)",
                    "auditor" => "auditor(a)",
                    _ => "administrador(a)",
                };
                vars.insert("role_label", label.to_string());
                vars.insert(
                    "admin_url",
                    "https://democracia.social.br/admin".to_string(),
                );
                "role_platform"
            }
        };

        let Some((subject, body)) = dsoc_db::email_templates::render(&db, key, &vars).await else {
            tracing::warn!(key, "template de designação ausente; e-mail não enviado");
            return;
        };
        if let Err(err) = crate::mailer::send_html(&cfg, &email, &subject, &body).await {
            tracing::error!(?err, "e-mail de designação de papel falhou");
        }
    });
}

async fn set_platform_role(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(citizen_id): Path<Uuid>,
    Json(body): Json<PlatformRoleBody>,
) -> Response {
    // The org is the one the caller PROVED admin in — never the body, never the
    // target's own org row (issue #8). `crates/app/src/caller.rs` states the
    // invariant; this handler used to be the counter-example to it.
    let admin = match crate::authz_ext::require_org_admin(&state.db, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let org_id = admin.org;

    // A citizen outside the caller's org is not theirs to promote. Without this the
    // gate would still hold (the caller IS an admin somewhere) while the write landed
    // on a stranger — so the check is on the TARGET, not only on the caller.
    let target_in_org: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM citizen WHERE id = $1 AND org_id = $2)")
            .bind(citizen_id)
            .bind(org_id)
            .fetch_one(&state.db)
            .await
            .unwrap_or(false);
    if !target_in_org {
        tracing::warn!(
            actor = %admin.citizen,
            org = %org_id,
            target = %citizen_id,
            "refused a platform-role grant to a citizen outside the caller's org"
        );
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::fail(
                "not_found",
                "Cidadão não encontrado.",
            )),
        )
            .into_response();
    }
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
        // Replace any previous role with this one. Two queries in
        // one tx — an sqlx prepared statement does not accept `DELETE ...; INSERT`.
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
        // Tell the person by e-mail (background, best-effort).
        notify_role_bg(
            &state.db,
            citizen_id,
            RoleNotice::Platform(body.role.clone()),
        );
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
    /// Mandatory when role ≠ 'none'.
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
        // Replaces any previous role of the citizen in that org to avoid
        // duplication (we could allow several parties per citizen, but
        // the UI treats it as 1:1). Two queries in one tx — an sqlx prepared
        // statement does not accept `DELETE ...; INSERT`.
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
        // Tell the person by e-mail (background, best-effort).
        let notice = if body.role == "admin" {
            RoleNotice::PartyAdmin(party.to_string())
        } else {
            RoleNotice::PartyModerador(party.to_string())
        };
        notify_role_bg(&state.db, citizen_id, notice);
    } else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::fail("invalid_input", "Role inválido.")),
        )
            .into_response();
    }
    (StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response()
}

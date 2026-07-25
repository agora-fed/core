//! `GET /api/v1/me/whoami` (0.52.0, mobile) — identidade CONSOLIDADA do usuário logado
//! num payload só, pra o app nativo decidir a navegação por papel sem precisar das 3
//! chamadas separadas (`/me` + `/me/admin-status` + `/me/mandate`).
//!
//! Compõe: perfil (reusa `ProfileService.get`) + mandato/binding (reusa
//! `MandateRegistry.find_my_mandate`) + papel de plataforma (`admin_role_binding`) +
//! papel de partido (`party_administrator`). Deriva `civic_type` (cidadao|candidato|politico).
//!
//! Auth: a identidade vem do header `x-dsoc-citizen-id` que o middleware `inject_identity`
//! injeta a partir da sessão/bearer (o cliente NUNCA envia esse header — é descartado na borda).

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use dsoc_core::ids::{CitizenId, OrgId};
use serde::Serialize;
use uuid::Uuid;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/me/whoami", get(whoami))
        .with_state(state)
}

#[derive(Serialize)]
struct WhoamiMandate {
    id: Uuid,
    office: String,
    is_candidate: bool,
    binding_level: String,
}

#[derive(Serialize)]
struct WhoamiDto {
    citizen_id: Uuid,
    handle: Option<String>,
    public_handle: String,
    display_name: Option<String>,
    avatar_url: Option<String>,
    verification_level: String,
    titulo_status: Option<String>,
    /// `owner` | `admin` | `auditor` | `null`.
    platform_role: Option<String>,
    /// Conveniência: `platform_role ∈ {owner, admin}`.
    is_admin: bool,
    /// `admin` | `moderador` | `null` (partido/diretório).
    party_role: Option<String>,
    /// Derivado: `cidadao` | `candidato` | `politico`.
    civic_type: String,
    /// Presente se o cidadão opera um mandato (político ou candidato autodeclarado).
    mandate: Option<WhoamiMandate>,
}

fn header_uuid(headers: &HeaderMap, key: &str) -> Option<Uuid> {
    headers
        .get(key)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(ApiResponse::<()>::fail(
            "unauthorized",
            "Autenticação necessária.",
        )),
    )
        .into_response()
}

async fn whoami(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(citizen_id) = header_uuid(&headers, "x-dsoc-citizen-id") else {
        return unauthorized();
    };
    // O middleware injeta org junto com o citizen; sem ela, cai no org default single-tenant.
    let org_id = header_uuid(&headers, "x-dsoc-org-id").unwrap_or_else(|| {
        Uuid::parse_str("11111111-1111-1111-1111-111111111111").expect("org uuid literal")
    });
    let citizen = CitizenId::from_uuid(citizen_id);
    let org = OrgId::from_uuid(org_id);

    // Perfil (nível de verificação + título) — reusa o serviço já testado.
    let profile = match dsoc_auth::profile::ProfileService::from_state(&state)
        .get(citizen, org)
        .await
    {
        Ok(p) => p,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::fail("storage_error", "Erro interno.")),
            )
                .into_response()
        }
    };

    // Mandato + nível do binding (político/candidato).
    let mandate = match dsoc_mandates::service::MandateRegistry::from_state(&state)
        .find_my_mandate(org, citizen)
        .await
    {
        Ok(Some((view, level))) => Some(WhoamiMandate {
            id: view.id.as_uuid(),
            office: view.office,
            is_candidate: view.is_candidate,
            binding_level: level,
        }),
        Ok(None) => None,
        Err(_) => None,
    };

    // Papel de plataforma (owner > admin > auditor).
    let platform_role: Option<String> = sqlx::query_scalar(
        r"SELECT role FROM admin_role_binding
           WHERE citizen_id = $1
           ORDER BY CASE role WHEN 'owner' THEN 0 WHEN 'admin' THEN 1 ELSE 2 END
           LIMIT 1",
    )
    .bind(citizen_id)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);
    let is_admin = matches!(platform_role.as_deref(), Some("owner") | Some("admin"));

    // Papel de partido (admin > moderador), só bindings aceitos.
    let party_role: Option<String> = sqlx::query_scalar(
        r"SELECT role FROM party_administrator
           WHERE citizen_id = $1 AND accepted_at IS NOT NULL
           ORDER BY CASE role WHEN 'admin' THEN 0 ELSE 1 END
           LIMIT 1",
    )
    .bind(citizen_id)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);

    let civic_type = match &mandate {
        Some(m) if m.is_candidate => "candidato",
        Some(_) => "politico",
        None => "cidadao",
    }
    .to_owned();

    let dto = WhoamiDto {
        citizen_id: profile.citizen_id,
        handle: profile.handle,
        public_handle: profile.public_handle,
        display_name: profile.display_name,
        avatar_url: profile.avatar_url,
        verification_level: profile.verification_level,
        titulo_status: profile.titulo_status,
        platform_role,
        is_admin,
        party_role,
        civic_type,
        mandate,
    };
    (StatusCode::OK, Json(ApiResponse::ok(dto))).into_response()
}

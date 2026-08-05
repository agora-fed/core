//! Permission gate for gateway handlers (R0.3 / ADR-0011).
//!
//! `require_permission(state, caller, key)` resolves the caller's effective [`Permissions`] in
//! their org (bound roles + implicit Base role) and checks a single `modulo.acao` key. It runs
//! INSIDE the handler, using the same `CallerId` (and therefore the same org) the handler acts
//! on — the ADR amendment: gate in the extractor, not in a router middleware (a mutation's org comes from the
//! CallerId, not from the body).
//!
//! This is the definitive replacement for the scattered `require_admin` helpers and the interim
//! gates shipped in the security queue (0.59.2/0.59.3). Migration of existing call sites is
//! incremental (R0.4 → R2).

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use dsoc_admin::AdminService;
use dsoc_api_contract::ApiResponse;
use dsoc_app::{AppState, CallerId};

/// Mounts `GET /me/permissions` — the caller's effective permission keys, so the front can
/// decide which management/moderation controls to render (it never gates on its own; the API
/// re-checks every action).
pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/me/permissions", get(my_permissions))
        .with_state(state)
}

async fn my_permissions(State(state): State<AppState>, caller: CallerId) -> Response {
    let svc = AdminService::from_state(&state);
    match svc.permissions_for(caller.org, caller.citizen).await {
        Ok(perms) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({
                "keys": perms.keys_sorted(),
                "is_administrator": perms.is_administrator(),
            }))),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(error = ?err, "my_permissions lookup failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::fail("http_500", "Erro interno.")),
            )
                .into_response()
        }
    }
}

/// The default org, used when the session middleware did not set an org header.
/// A single-org install is the common case today; a missing header must never
/// widen the check, so this narrows it to one specific tenant rather than none.
pub(crate) const DEFAULT_ORG_UUID: uuid::Uuid = uuid::uuid!("11111111-1111-1111-1111-111111111111");

/// An admin caller, proven to hold `owner`/`admin` IN A SPECIFIC ORG (issue #8).
///
/// The org is carried out of the gate on purpose: a handler that needs a target org
/// takes it from here, so it cannot accidentally read one from the request body.
#[derive(Debug, Clone, Copy)]
pub struct AdminCaller {
    /// The authenticated citizen.
    pub citizen: uuid::Uuid,
    /// The org the admin binding was proven in — and the ONLY org this caller may act on.
    pub org: uuid::Uuid,
}

/// The one org-scoped `admin_role_binding` gate (issue #8).
///
/// Before this existed, sixteen modules each carried a private copy of the check and
/// fifteen of them omitted `org_id` — so an owner of ANY org passed the gate
/// everywhere, and the multi-tenant boundary was a naming convention. The table has
/// been per-org since 0150 (`UNIQUE (org_id, citizen_id, role)`); only the queries
/// forgot.
///
/// Both identifiers come from the gateway-set headers, which the public ingress
/// strips from clients (see `dsoc_app::CallerId`) — never from the request body.
///
/// A DB failure denies. This gate is the boundary between tenants, so it fails
/// CLOSED: an unavailable database must not read as "everyone is an admin".
///
/// # Errors
/// 401 when there is no authenticated caller; 403 when they hold no owner/admin
/// binding in their own org.
pub async fn require_org_admin(
    db: &sqlx::PgPool,
    headers: &axum::http::HeaderMap,
) -> Result<AdminCaller, Response> {
    let header_uuid = |name: &str| -> Option<uuid::Uuid> {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse().ok())
    };
    let Some(citizen) = header_uuid("x-dsoc-citizen-id") else {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ApiResponse::<()>::fail(
                "unauthorized",
                "Autenticação necessária.",
            )),
        )
            .into_response());
    };
    let org = header_uuid("x-dsoc-org-id").unwrap_or(DEFAULT_ORG_UUID);

    let is_admin: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM admin_role_binding \
          WHERE org_id = $1 AND citizen_id = $2 AND role IN ('owner','admin'))",
    )
    .bind(org)
    .bind(citizen)
    .fetch_one(db)
    .await
    .unwrap_or(false);

    if is_admin {
        Ok(AdminCaller { citizen, org })
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(ApiResponse::<()>::fail(
                "forbidden",
                "Acesso restrito a administradores.",
            )),
        )
            .into_response())
    }
}

/// Assert the caller holds `key` in their org. `Ok(())` to proceed; `Err(response)` is a ready
/// 403 (missing permission) or 500 (lookup failure) for the handler to return directly.
///
/// `administrator` satisfies every key (handled inside [`dsoc_admin::permissions::Permissions`]).
pub async fn require_permission(
    state: &AppState,
    caller: CallerId,
    key: &str,
) -> Result<(), Response> {
    let svc = AdminService::from_state(state);
    match svc.permissions_for(caller.org, caller.citizen).await {
        Ok(perms) if perms.can(key) => Ok(()),
        Ok(_) => Err((
            StatusCode::FORBIDDEN,
            Json(ApiResponse::<()>::fail(
                "http_403",
                "Você não tem permissão para esta ação.",
            )),
        )
            .into_response()),
        Err(err) => {
            tracing::error!(error = ?err, key, "require_permission lookup failed");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::fail("http_500", "Erro interno.")),
            )
                .into_response())
        }
    }
}

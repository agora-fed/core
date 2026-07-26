//! Permission gate for gateway handlers (R0.3 / ADR-0011).
//!
//! `require_permission(state, caller, key)` resolves the caller's effective [`Permissions`] in
//! their org (bound roles + implicit Base role) and checks a single `modulo.acao` key. It runs
//! INSIDE the handler, using the same `CallerId` (and therefore the same org) the handler acts
//! on — the ADR emenda: gate no extractor, não em middleware de router (o org de mutação vem do
//! CallerId, não do body).
//!
//! This is the definitive replacement for the scattered `require_admin` helpers and the interim
//! gates shipped in the security queue (0.59.2/0.59.3). Migration of existing call sites is
//! incremental (R0.4 → R2).

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use dsoc_admin::AdminService;
use dsoc_api_contract::ApiResponse;
use dsoc_app::{AppState, CallerId};

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

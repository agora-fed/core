//! DIOGENES add-on wiring (agora-fed/diogenes).
//!
//! DIOGENES has no dependency on AGORA crates (its ADR-0002). This module is the host side:
//! it implements `diogenes::Identity` on top of the gateway's own identity and permissions.
//!
//! * The caller comes from `x-dsoc-citizen-id` / `x-dsoc-org-id`. `inject_identity` strips any
//!   client copy and sets them only from a real session, so the API router MUST be merged
//!   inside the `api` chain (under the identity layer).
//! * Permissions: `diogenes.rooms.create`, `diogenes.sync.operate` (role grants, or
//!   `administrator`).
//! * Module gate: `module.diogenes`, default off per org.

use std::sync::Arc;

use async_trait::async_trait;
use axum::http::request::Parts;
use axum::Router;
use dsoc_admin::AdminService;
use dsoc_app::AppState;
use dsoc_core::ids::{CitizenId, OrgId};
use uuid::Uuid;

use crate::module_catalog::{
    DIOGENES_CREATE_ROOMS as PERM_CREATE_ROOMS, DIOGENES_OPERATE_SYNC as PERM_OPERATE_SYNC,
};

const MODULE_ID: &str = "diogenes";

/// The host identity adapter.
#[derive(Clone)]
pub struct HostIdentity {
    state: AppState,
}

impl std::fmt::Debug for HostIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostIdentity").finish_non_exhaustive()
    }
}

fn header_uuid(parts: &Parts, name: &str) -> Option<Uuid> {
    parts.headers.get(name)?.to_str().ok()?.parse().ok()
}

#[async_trait]
impl diogenes::Identity for HostIdentity {
    async fn actor(&self, parts: &Parts) -> Option<diogenes::Actor> {
        let citizen = header_uuid(parts, "x-dsoc-citizen-id")?;
        let org = header_uuid(parts, "x-dsoc-org-id")?;
        let display_name: String = sqlx::query_scalar(
            "SELECT COALESCE(NULLIF(display_name, ''), handle, 'Participante') FROM citizen WHERE id = $1",
        )
        .bind(citizen)
        .fetch_optional(&self.state.db)
        .await
        .map_err(|err| tracing::error!(error = %err, "diogenes caller lookup failed"))
        .ok()
        .flatten()?;
        let perms = AdminService::from_state(&self.state)
            .permissions_for(OrgId::from_uuid(org), CitizenId::from_uuid(citizen))
            .await
            .map_err(|err| tracing::error!(error = ?err, "diogenes permissions lookup failed"))
            .ok();
        let can = |key: &str| perms.as_ref().is_some_and(|p| p.can(key));
        // The Chamber/TSE data has no org: it is shared by the whole installation. Only the
        // operator org (DIOGENES_OPERATOR_ORG) may trigger the sync by hand, so an
        // administrator of another tenant org cannot.
        Some(diogenes::Actor {
            user_id: citizen,
            org_id: org,
            display_name,
            can_create_rooms: can(PERM_CREATE_ROOMS),
            can_operate_sync: can(PERM_OPERATE_SYNC) && is_operator_org(org),
        })
    }

    async fn find_user(&self, org_id: Uuid, handle: &str) -> Option<diogenes::UserRef> {
        let row: Option<(Uuid, String)> = sqlx::query_as(
            "SELECT id, COALESCE(NULLIF(display_name, ''), handle)
               FROM citizen WHERE org_id = $1 AND lower(handle) = lower($2)",
        )
        .bind(org_id)
        .bind(handle)
        .fetch_optional(&self.state.db)
        .await
        .map_err(|err| tracing::error!(error = %err, "diogenes user lookup failed"))
        .ok()
        .flatten();
        row.map(|(user_id, display_name)| diogenes::UserRef {
            user_id,
            display_name,
        })
    }

    async fn module_active(&self, org_id: Uuid) -> bool {
        crate::module_gate::require_module(&self.state, org_id, MODULE_ID)
            .await
            .is_ok()
    }
}

/// True when `org` is the operator org of the installation (`DIOGENES_OPERATOR_ORG`).
/// Unset or invalid = no org can trigger the sync by hand (the scheduler still runs).
fn is_operator_org(org: Uuid) -> bool {
    std::env::var("DIOGENES_OPERATOR_ORG")
        .ok()
        .and_then(|v| v.parse::<Uuid>().ok())
        .is_some_and(|op| op == org)
}

/// The DIOGENES JSON API (`/diogenes/...`). Merge it INSIDE the `/api/v1` chain.
pub fn routes(state: AppState) -> Router<()> {
    let db = state.db.clone();
    let identity = Arc::new(HostIdentity { state });
    diogenes::api_router(diogenes::DiogenesState::new(db, identity))
}

/// The articulation area pages (`/articulacao/`). Merge at the site root.
pub fn web_routes() -> Router<()> {
    diogenes::web_router()
}

/// Apply the DIOGENES schema and start the sync scheduler.
///
/// The sync is OPT-IN: it runs only with `DIOGENES_SYNC_ENABLED=true` (the Pindorama deploy sets
/// it). An installation that never turns the module on makes no call to the Câmara or the TSE.
/// The schema is additive (`diogenes_*` tables only). A schema error is logged and the gateway
/// keeps running: the add-on endpoints then fail on their own, the rest is unaffected.
pub async fn start(state: &AppState) {
    if let Err(err) = diogenes::migrate(&state.db).await {
        tracing::error!(error = %err, "diogenes migrations failed; the add-on is unavailable");
        return;
    }
    let enabled = std::env::var("DIOGENES_SYNC_ENABLED").is_ok_and(|v| v == "true");
    if enabled {
        let _handle = diogenes::sync::spawn_scheduler(
            state.db.clone(),
            diogenes::sync::SyncConfig::from_env(),
        );
        tracing::info!("diogenes sync scheduler started");
    }
}

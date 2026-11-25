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
//! * Pages: drawn by the site (`web/src/pages/articulacao`) with the design system; the
//!   crate's own pages are not mounted (ADR-0020, diogenes ADR-0007).
//! * New members are told through `diogenes_notice` (bell, web push, e-mail).

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

/// Log a failed lookup and turn it into `None`. The add-on then answers as if the row did
/// not exist (no identity, no permission), which fails closed.
fn logged<T, E: std::fmt::Debug>(result: Result<T, E>, what: &str) -> Option<T> {
    result
        .map_err(|err| tracing::error!(error = ?err, "diogenes {what} failed"))
        .ok()
}

#[async_trait]
impl diogenes::Identity for HostIdentity {
    async fn actor(&self, parts: &Parts) -> Option<diogenes::Actor> {
        let citizen = header_uuid(parts, "x-dsoc-citizen-id")?;
        let org = header_uuid(parts, "x-dsoc-org-id")?;
        let lookup = sqlx::query_scalar::<_, String>(
            "SELECT COALESCE(NULLIF(display_name, ''), '@' || handle, 'Participante') FROM citizen WHERE id = $1",
        )
        .bind(citizen)
        .fetch_optional(&self.state.db)
        .await;
        let display_name = logged(lookup, "caller lookup").flatten()?;
        let perms = AdminService::from_state(&self.state)
            .permissions_for(OrgId::from_uuid(org), CitizenId::from_uuid(citizen))
            .await;
        let perms = logged(perms, "permissions lookup");
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
        let lookup = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT id, COALESCE(NULLIF(display_name, ''), '@' || handle)
               FROM citizen WHERE org_id = $1 AND lower(handle) = lower($2)",
        )
        .bind(org_id)
        .bind(handle)
        .fetch_optional(&self.state.db)
        .await;
        logged(lookup, "user lookup")
            .flatten()
            .map(|(user_id, display_name)| diogenes::UserRef {
                user_id,
                display_name,
            })
    }

    async fn module_active(&self, org_id: Uuid) -> bool {
        crate::module_gate::require_module(&self.state, org_id, MODULE_ID)
            .await
            .is_ok()
    }

    async fn member_added(&self, notice: diogenes::MemberAdded) {
        crate::diogenes_notice::spawn(self.state.db.clone(), public_origin(), notice);
    }
}

/// Public origin of the site, for links in notices (`PUBLIC_ORIGIN`).
fn public_origin() -> String {
    std::env::var("PUBLIC_ORIGIN").unwrap_or_else(|_| "https://democracia.social.br".to_owned())
}

/// True when `org` is the operator org of the installation (`DIOGENES_OPERATOR_ORG`).
/// Unset or invalid = no org can trigger the sync by hand (the scheduler still runs).
fn is_operator_org(org: Uuid) -> bool {
    operator_org_matches(std::env::var("DIOGENES_OPERATOR_ORG").ok().as_deref(), org)
}

fn operator_org_matches(configured: Option<&str>, org: Uuid) -> bool {
    configured
        .and_then(|v| v.parse::<Uuid>().ok())
        .is_some_and(|op| op == org)
}

/// The DIOGENES JSON API (`/diogenes/...`). Merge it INSIDE the `/api/v1` chain.
pub fn routes(state: AppState) -> Router<()> {
    let db = state.db.clone();
    let identity = Arc::new(HostIdentity { state });
    diogenes::api_router(diogenes::DiogenesState::new(db, identity))
        .layer(axum::middleware::from_fn(public_read_limit))
}

/// Reads per minute of one caller on the public campaign routes.
const PUBLIC_READS_PER_MIN: u32 = 120;

/// True for the campaign routes that anonymous callers can read (diogenes ADR-0009/0010).
fn is_public_read(method: &axum::http::Method, path: &str) -> bool {
    method == axum::http::Method::GET
        && (path.ends_with("/diogenes/campaigns")
            || (path.contains("/diogenes/deputies/") && path.ends_with("/campaign")))
}

/// The public campaign routes need no session, so the mutating-only limiter of the host
/// does not cover them. Limit them per citizen, or per client address when anonymous.
async fn public_read_limit(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if is_public_read(req.method(), req.uri().path()) {
        let header = |name: &str| {
            req.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
        };
        let key = header("x-dsoc-citizen-id")
            .map(|c| format!("diogenes-read:c:{c}"))
            .or_else(|| header(crate::CLIENT_IP_HEADER).map(|ip| format!("diogenes-read:ip:{ip}")));
        if let Some(key) = key {
            if crate::rate_limit::over_limit(&key, PUBLIC_READS_PER_MIN) {
                return crate::rate_limit::too_many();
            }
        }
    }
    next.run(req).await
}

/// Apply the DIOGENES schema and start the sync scheduler.
///
/// The sync is OPT-IN: it runs only with `DIOGENES_SYNC_ENABLED=true` (the Pindorama deploy sets
/// it). An installation that never turns the module on makes no call to the Câmara or the TSE.
pub async fn start(state: &AppState) {
    let _scheduler = start_with(
        state,
        sync_enabled(std::env::var("DIOGENES_SYNC_ENABLED").ok()),
    )
    .await;
}

/// `DIOGENES_SYNC_ENABLED` turns the sync on only with the exact value `true`.
fn sync_enabled(value: Option<String>) -> bool {
    value.is_some_and(|v| v == "true")
}

/// [`start`] with the sync decision made by the caller. Returns the scheduler task when it
/// started.
///
/// The schema is additive (`diogenes_*` tables only). A schema error is logged and the gateway
/// keeps running: the add-on endpoints then fail on their own, the rest is unaffected.
pub async fn start_with(state: &AppState, sync: bool) -> Option<tokio::task::JoinHandle<()>> {
    if let Err(err) = diogenes::migrate(&state.db).await {
        tracing::error!(error = %err, "diogenes migrations failed; the add-on is unavailable");
        return None;
    }
    if !sync {
        return None;
    }
    let handle =
        diogenes::sync::spawn_scheduler(state.db.clone(), diogenes::sync::SyncConfig::from_env());
    tracing::info!("diogenes sync scheduler started");
    Some(handle)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use axum::http::Request;
    use diogenes::Identity as _;
    use dsoc_core::{Clock, SystemClock};

    /// A state whose database is unreachable: every query fails fast.
    fn offline_state() -> AppState {
        let db = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_millis(200))
            .connect_lazy("postgres://nobody@[::1]:9/none")
            .unwrap();
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let bus = Arc::new(dsoc_events::PgEventBus::new(db.clone()));
        let authz = dsoc_auth::authorization(db.clone(), clock.clone(), bus.clone());
        AppState {
            db,
            bus,
            authz,
            clock,
            storage: None,
        }
    }

    #[test]
    fn only_the_campaign_reads_are_public() {
        use axum::http::Method;
        assert!(is_public_read(&Method::GET, "/api/v1/diogenes/campaigns"));
        assert!(is_public_read(&Method::GET, "/api/v1/diogenes/deputies/204554/campaign"));
        assert!(!is_public_read(&Method::POST, "/api/v1/diogenes/campaigns"));
        assert!(!is_public_read(&Method::GET, "/api/v1/diogenes/rooms"));
        assert!(!is_public_read(&Method::GET, "/api/v1/diogenes/scenarios/x/donors"));
    }

    #[tokio::test]
    async fn anonymous_campaign_reads_are_limited_per_address() {
        use tower::ServiceExt;
        let app = Router::new()
            .route("/api/v1/diogenes/campaigns", axum::routing::get(|| async { "ok" }))
            .layer(axum::middleware::from_fn(public_read_limit));
        let ip = format!("203.0.113.{}", std::process::id() % 250);
        let call = || {
            axum::http::Request::get("/api/v1/diogenes/campaigns")
                .header(crate::CLIENT_IP_HEADER, ip.clone())
                .body(axum::body::Body::empty())
                .unwrap()
        };
        // Twice the cap plus one: one minute window gets more than the cap, even when the
        // loop crosses a minute boundary.
        let mut limited = false;
        for _ in 0..=2 * PUBLIC_READS_PER_MIN {
            let status = app.clone().oneshot(call()).await.unwrap().status();
            limited |= status == axum::http::StatusCode::TOO_MANY_REQUESTS;
        }
        assert!(limited);
    }

    fn parts(headers: &[(&str, String)]) -> Parts {
        let mut req = Request::builder();
        for (k, v) in headers {
            req = req.header(*k, v);
        }
        req.body(()).unwrap().into_parts().0
    }

    #[tokio::test]
    async fn debug_does_not_print_the_state() {
        let out = format!(
            "{:?}",
            HostIdentity {
                state: offline_state()
            }
        );
        assert_eq!(out, "HostIdentity { .. }");
    }

    #[test]
    fn operator_org_needs_an_exact_valid_id() {
        let org = Uuid::now_v7();
        assert!(operator_org_matches(Some(&org.to_string()), org));
        assert!(!operator_org_matches(
            Some(&Uuid::now_v7().to_string()),
            org
        ));
        assert!(!operator_org_matches(Some("not-a-uuid"), org));
        assert!(!operator_org_matches(None, org));
        // Unset in the test environment: no org may trigger the sync by hand.
        assert!(!is_operator_org(org));
    }

    #[test]
    fn sync_runs_only_with_the_exact_value_true() {
        assert!(sync_enabled(Some("true".to_owned())));
        assert!(!sync_enabled(Some("TRUE".to_owned())));
        assert!(!sync_enabled(Some("1".to_owned())));
        assert!(!sync_enabled(None));
    }

    #[tokio::test]
    async fn missing_identity_headers_give_no_actor() {
        let id = HostIdentity {
            state: offline_state(),
        };
        assert!(id.actor(&parts(&[])).await.is_none());
        let bad = [
            ("x-dsoc-citizen-id", "x".to_owned()),
            ("x-dsoc-org-id", "y".to_owned()),
        ];
        assert!(id.actor(&parts(&bad)).await.is_none());
    }

    #[tokio::test]
    async fn a_database_failure_fails_closed() {
        let id = HostIdentity {
            state: offline_state(),
        };
        let caller = [
            ("x-dsoc-citizen-id", Uuid::now_v7().to_string()),
            ("x-dsoc-org-id", Uuid::now_v7().to_string()),
        ];
        assert!(id.actor(&parts(&caller)).await.is_none());
        assert!(id.find_user(Uuid::now_v7(), "ana").await.is_none());
        assert!(!id.module_active(Uuid::now_v7()).await);
    }

    #[tokio::test]
    async fn a_schema_failure_starts_nothing() {
        assert!(start_with(&offline_state(), true).await.is_none());
    }
}

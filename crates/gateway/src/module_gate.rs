//! Per-organization module gate (R0.5 / #42, ADR-0011).
//!
//! A module is ACTIVE in an org when: it is `core` (always), OR it has a row in `admin_feature_flag`
//! with `enabled=true`, OR it has no row and the manifest says `default_enabled`. `require_module`
//! runs INSIDE the handler with the `CallerId`'s org (amendment P3.1 — never a router middleware for
//! a mutation, whose org would come from the body). Flags are cached for 30s (amendment P3.3/P4.1 — cache
//! configuration ONLY, NEVER role grants).
//!
//! `GET /orgs/{org}/modules` is PUBLIC by design (amendment P6.3): it exposes only the modules' effective
//! state, not the raw flag rows (those stay behind flags.manage in the admin).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use dsoc_api_contract::ApiResponse;
use dsoc_app::manifest::ModuleManifest;
use dsoc_app::AppState;
use serde::Serialize;
use uuid::Uuid;

use crate::module_catalog;

const FLAG_TTL: Duration = Duration::from_secs(30);

/// What the flag lookup found — THREE distinct states (issue #18).
///
/// These used to collapse into one `Option<bool>` via `.ok().flatten()`, which made
/// "the database is unreachable" indistinguishable from "there is no row". Since a
/// missing row means "use the manifest default", and 20 of the 26 manifests default
/// to ON, a momentary DB error silently RE-ENABLED a module an admin had turned off —
/// and the result was cached, so the fail-open outlived the blip by 30s.
///
/// The type exists so the error case cannot be ignored at the match site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlagState {
    /// A row exists and says this.
    Row(bool),
    /// No row for this (org, key) — the manifest default applies.
    Absent,
    /// The lookup itself failed. Deny, and never cache.
    Unavailable,
}

/// Cache (org, flag_key) → (read_at, state). Only CONCLUSIVE reads land here;
/// [`FlagState::Unavailable`] is deliberately never cached, so recovery is immediate
/// rather than delayed by the TTL.
static FLAG_CACHE: std::sync::LazyLock<
    tokio::sync::RwLock<HashMap<(Uuid, String), (Instant, FlagState)>>,
> = std::sync::LazyLock::new(|| tokio::sync::RwLock::new(HashMap::new()));

/// Read `enabled` of a `module.<id>` for an org, with a TTL cache over conclusive reads.
async fn flag_state(state: &AppState, org: Uuid, key: &str) -> FlagState {
    let ck = (org, key.to_owned());
    if let Some((at, val)) = FLAG_CACHE.read().await.get(&ck) {
        if at.elapsed() < FLAG_TTL {
            return *val;
        }
    }
    let found = sqlx::query_scalar::<_, bool>(
        "SELECT enabled FROM admin_feature_flag WHERE org_id = $1 AND key = $2",
    )
    .bind(org)
    .bind(key)
    .fetch_optional(&state.db)
    .await;
    let val = match found {
        Ok(Some(enabled)) => FlagState::Row(enabled),
        Ok(None) => FlagState::Absent,
        Err(err) => {
            tracing::warn!(error = ?err, org = %org, key, "module flag lookup failed — denying");
            return FlagState::Unavailable;
        }
    };
    FLAG_CACHE.write().await.insert(ck, (Instant::now(), val));
    val
}

/// Resolve a lookup result into "is this module active?".
///
/// Pure on purpose: this is the decision the gate turns on, and it is the part worth
/// pinning in tests without a database.
fn effective_active(found: FlagState, m: &ModuleManifest) -> bool {
    if m.core {
        return true;
    }
    match found {
        FlagState::Row(enabled) => enabled,
        FlagState::Absent => m.default_enabled,
        // FAIL CLOSED. An authorization primitive that opens under load is worse
        // than one that closes: a denied request is visible and retryable, a
        // wrongly-granted one is neither.
        FlagState::Unavailable => false,
    }
}

/// Effective state of a module in an org.
async fn module_active(state: &AppState, org: Uuid, m: &ModuleManifest) -> bool {
    if m.core {
        return true;
    }
    effective_active(flag_state(state, org, m.flag_key).await, m)
}

/// Gate for a handler: `Ok(())` when the module is active in the org, otherwise a ready 404 (a module off =
/// the route "does not exist" for that org). An unknown id → Ok (never block what is not a module).
pub async fn require_module(state: &AppState, org: Uuid, module_id: &str) -> Result<(), Response> {
    let Some(m) = module_catalog::find(module_id) else {
        return Ok(());
    };
    if module_active(state, org, m).await {
        Ok(())
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::fail(
                "module_disabled",
                "Este recurso não está disponível nesta organização.",
            )),
        )
            .into_response())
    }
}

// ---------------------------------------------------------------------------
// GET /orgs/{org}/modules — effective state (public)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ModuleStateDto {
    id: String,
    title: String,
    active: bool,
    core: bool,
    gateable: bool,
}

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/orgs/{org}/modules", get(list_modules))
        .with_state(state)
}

async fn list_modules(State(state): State<AppState>, Path(org): Path<Uuid>) -> Response {
    let mut out = Vec::with_capacity(module_catalog::CATALOG.len());
    for m in module_catalog::CATALOG {
        out.push(ModuleStateDto {
            id: m.id.to_owned(),
            title: m.title.to_owned(),
            active: module_active(&state, org, m).await,
            core: m.core,
            gateable: m.gateable,
        });
    }
    (StatusCode::OK, Json(ApiResponse::ok(out))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(core: bool, default_enabled: bool) -> ModuleManifest {
        ModuleManifest {
            id: "t",
            title: "T",
            core,
            flag_key: "module.t",
            default_enabled,
            gateable: true,
            depends_on: &[],
            permissions: &[],
            nav: &[],
        }
    }

    #[test]
    fn a_row_wins_over_the_manifest_default() {
        // An admin's explicit choice beats the ship default, in both directions.
        assert!(!effective_active(
            FlagState::Row(false),
            &manifest(false, true)
        ));
        assert!(effective_active(
            FlagState::Row(true),
            &manifest(false, false)
        ));
    }

    #[test]
    fn no_row_falls_back_to_the_manifest_default() {
        assert!(effective_active(FlagState::Absent, &manifest(false, true)));
        assert!(!effective_active(
            FlagState::Absent,
            &manifest(false, false)
        ));
    }

    #[test]
    fn a_failed_lookup_denies_even_when_the_default_is_on() {
        // THE BUG: `Unavailable` used to be indistinguishable from `Absent`, so a DB
        // blip resolved to the manifest default — ON for 20 of the 26 modules.
        assert!(
            !effective_active(FlagState::Unavailable, &manifest(false, true)),
            "a DB error must not re-enable a module"
        );
        assert!(!effective_active(
            FlagState::Unavailable,
            &manifest(false, false)
        ));
    }

    #[test]
    fn core_modules_stay_on_through_a_failed_lookup() {
        // Failing closed must not take down `auth` — core is not gateable at all,
        // so its answer cannot depend on a flag read.
        for found in [
            FlagState::Unavailable,
            FlagState::Absent,
            FlagState::Row(false),
        ] {
            assert!(
                effective_active(found, &manifest(true, false)),
                "core module must stay active for {found:?}"
            );
        }
    }
}

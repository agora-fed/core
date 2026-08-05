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

/// Cache (org, flag_key) → (read_at, Option<enabled>). `None` = no row (uses the manifest default).
static FLAG_CACHE: std::sync::LazyLock<
    tokio::sync::RwLock<HashMap<(Uuid, String), (Instant, Option<bool>)>>,
> = std::sync::LazyLock::new(|| tokio::sync::RwLock::new(HashMap::new()));

/// Read `enabled` of a `module.<id>` for an org, with a TTL cache. `None` when there is no row.
async fn flag_enabled(state: &AppState, org: Uuid, key: &str) -> Option<bool> {
    let ck = (org, key.to_owned());
    if let Some((at, val)) = FLAG_CACHE.read().await.get(&ck) {
        if at.elapsed() < FLAG_TTL {
            return *val;
        }
    }
    let val: Option<bool> =
        sqlx::query_scalar("SELECT enabled FROM admin_feature_flag WHERE org_id = $1 AND key = $2")
            .bind(org)
            .bind(key)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    FLAG_CACHE.write().await.insert(ck, (Instant::now(), val));
    val
}

/// Effective state of a module in an org.
async fn module_active(state: &AppState, org: Uuid, m: &ModuleManifest) -> bool {
    if m.core {
        return true;
    }
    match flag_enabled(state, org, m.flag_key).await {
        Some(enabled) => enabled,
        None => m.default_enabled,
    }
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

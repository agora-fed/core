//! Gate de módulo por organização (R0.5 / #42, ADR-0011).
//!
//! Um módulo está ATIVO numa org quando: é `core` (sempre), OU tem linha em `admin_feature_flag`
//! com `enabled=true`, OU não tem linha e o manifesto diz `default_enabled`. `require_module`
//! roda DENTRO do handler com o org do `CallerId` (emenda P3.1 — nunca middleware de router pra
//! mutação, cujo org viria do body). Flags são cacheadas por 30s (emenda P3.3/P4.1 — cacheia só
//! configuração, NUNCA grants de papel).
//!
//! `GET /orgs/{org}/modules` é PÚBLICO por design (emenda P6.3): expõe só o estado efetivo dos
//! módulos, não as linhas cruas de flags (essas ficam atrás de flags.manage no admin).

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

/// Cache (org, flag_key) → (lido_em, Option<enabled>). `None` = sem linha (usa default do manifesto).
static FLAG_CACHE: std::sync::LazyLock<
    tokio::sync::RwLock<HashMap<(Uuid, String), (Instant, Option<bool>)>>,
> = std::sync::LazyLock::new(|| tokio::sync::RwLock::new(HashMap::new()));

/// Lê `enabled` de um `module.<id>` pra uma org, com cache TTL. `None` quando não há linha.
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

/// Estado efetivo de um módulo numa org.
async fn module_active(state: &AppState, org: Uuid, m: &ModuleManifest) -> bool {
    if m.core {
        return true;
    }
    match flag_enabled(state, org, m.flag_key).await {
        Some(enabled) => enabled,
        None => m.default_enabled,
    }
}

/// Gate pra um handler: `Ok(())` se o módulo está ativo na org, senão 404 pronto (módulo off =
/// a rota "não existe" pra essa org). Id desconhecido → Ok (não bloqueia o que não é módulo).
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
// GET /orgs/{org}/modules — estado efetivo (público)
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

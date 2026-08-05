//! # Rate-limit de escrita (0.42.0) — defesa contra spam/brigading.
//!
//! Until now only login/contact/audience/federated-post had a limit. Sensitive
//! writes (votes, proposals, comments, reports, campaign, electoral registry)
//! were open to scripts. This middleware puts a GLOBAL cap on mutations
//! per minute per caller across the state-changing methods (POST/PUT/PATCH/DELETE).
//!
//! - Target: AUTHENTICATED calls (a logged-in account spamming). The key is the
//!   citizen (`x-dsoc-citizen-id`, already set by `inject_identity` — which is why
//!   this layer runs AFTER it). Anonymous requests pass: the anonymous+mutating
//!   endpoints (login/register/respond/contact) already carry their own limit,
//!   and votes/proposals/etc. require a session (401 without one).
//! - A fixed 1-minute window, an in-memory counter (single process; a reset on
//!   pod restart is acceptable for abuse prevention). The cap is configurable via
//!   `RATE_LIMIT_WRITES_PER_MIN` (default 30).
//! - It counts mutating methods only; GET/HEAD pass freely.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::Request;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use dsoc_api_contract::ApiResponse;

/// (janela-em-minutos-epoch, contagem) por chave.
type Buckets = HashMap<String, (u64, u32)>;
static STATE: LazyLock<Mutex<Buckets>> = LazyLock::new(|| Mutex::new(HashMap::new()));

const DEFAULT_PER_MIN: u32 = 30;

fn limit_per_min() -> u32 {
    std::env::var("RATE_LIMIT_WRITES_PER_MIN")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&n: &u32| n > 0)
        .unwrap_or(DEFAULT_PER_MIN)
}

fn current_minute() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / 60)
        .unwrap_or(0)
}

/// The authenticated citizen's key, or `None` for anonymous (which we do not limit here).
fn caller_key(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-dsoc-citizen-id")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .map(|c| format!("c:{c}"))
}

/// Registra um hit na janela atual e diz se estourou o teto.
fn hit(key: &str, per_min: u32) -> bool {
    let minute = current_minute();
    let mut map = match STATE.lock() {
        Ok(m) => m,
        Err(p) => p.into_inner(),
    };
    // Poda barata: se o mapa cresceu muito, remove entradas de janelas velhas.
    if map.len() > 10_000 {
        map.retain(|_, (w, _)| *w == minute);
    }
    let entry = map.entry(key.to_owned()).or_insert((minute, 0));
    if entry.0 != minute {
        *entry = (minute, 0);
    }
    entry.1 += 1;
    entry.1 > per_min
}

fn too_many() -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        Json(ApiResponse::<()>::fail(
            "rate_limited",
            "Muitas ações em pouco tempo. Aguarde um minuto e tente de novo.",
        )),
    )
        .into_response()
}

/// Middleware. Runs as the INNERMOST layer (after inject_identity),
/// so it already sees the `x-dsoc-citizen-id` resolved from the session.
pub async fn rate_limit_middleware(req: Request, next: Next) -> Response {
    let mutating = matches!(
        *req.method(),
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    );
    if mutating {
        if let Some(key) = caller_key(req.headers()) {
            if hit(&key, limit_per_min()) {
                return too_many();
            }
        }
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_after_limit_in_same_window() {
        let key = format!("test:{}", current_minute());
        // With a cap of 3: the first 3 pass (false), the 4th exceeds (true).
        assert!(!hit(&key, 3));
        assert!(!hit(&key, 3));
        assert!(!hit(&key, 3));
        assert!(hit(&key, 3));
    }
}

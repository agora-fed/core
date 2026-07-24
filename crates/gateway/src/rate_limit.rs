//! # Rate-limit de escrita (0.42.0) — defesa contra spam/brigading.
//!
//! Até aqui só login/contato/audiência/post-federado tinham limite. Escritas
//! sensíveis (votos, propostas, comentários, denúncias, campanha, título)
//! ficavam abertas a scripts. Este middleware põe um teto GLOBAL de mutações
//! por minuto por chamador nos métodos que mudam estado (POST/PUT/PATCH/DELETE).
//!
//! - Alvo: chamadas AUTENTICADAS (uma conta logada spammando). A chave é o
//!   cidadão (`x-dsoc-citizen-id`, já setado pelo `inject_identity` — por isso
//!   esta camada roda DEPOIS dele). Requisições anônimas passam: os endpoints
//!   anônimos+mutantes (login/register/respond/contato) já têm limite próprio,
//!   e votos/propostas/etc. exigem sessão (401 sem ela).
//! - Janela fixa de 1 minuto, contador em memória (processo único; reset no
//!   restart do pod é aceitável pra prevenção de abuso). Teto configurável via
//!   `RATE_LIMIT_WRITES_PER_MIN` (default 30).
//! - Só conta métodos mutantes; GET/HEAD passam livres.

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

/// Chave do cidadão autenticado, ou `None` para anônimo (que não limitamos aqui).
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

/// Middleware. Roda como a camada MAIS INTERNA (depois do inject_identity),
/// então já enxerga o `x-dsoc-citizen-id` resolvido da sessão.
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
        // Com teto 3: os 3 primeiros passam (false), o 4º estoura (true).
        assert!(!hit(&key, 3));
        assert!(!hit(&key, 3));
        assert!(!hit(&key, 3));
        assert!(hit(&key, 3));
    }
}

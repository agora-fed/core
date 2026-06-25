//! Axum surface for the event bus. A single admin/health endpoint reports queue depth so
//! operators can see backlog per topic. The crate never binds a socket — the gateway owns the
//! IPv6 bind (CONTRIBUTING.md "Wiring conventions", ADR-0004).

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use utoipa::ToSchema;

use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use dsoc_core::Error;

use crate::domain::{self, QueueDepth};
use crate::service::EventQueue;

/// Pending deliveries on one topic (public admin view).
#[derive(Debug, Serialize, ToSchema)]
struct TopicDepthView {
    /// Routing topic string.
    topic: String,
    /// Count of unprocessed deliveries.
    pending: i64,
}

/// Queue-depth health report across every topic.
#[derive(Debug, Serialize, ToSchema)]
struct QueueHealth {
    /// Total unprocessed deliveries across all topics.
    total_pending: i64,
    /// Per-topic breakdown.
    topics: Vec<TopicDepthView>,
}

impl QueueHealth {
    fn from_depths(depths: Vec<QueueDepth>) -> Self {
        let total_pending = domain::total_pending(&depths);
        let topics = depths
            .into_iter()
            .map(|d| TopicDepthView {
                topic: d.topic,
                pending: d.pending,
            })
            .collect();
        Self {
            total_pending,
            topics,
        }
    }
}

/// Map the canonical [`Error`] to an HTTP status (mirrors the gateway's rendering).
fn status_for(err: &Error) -> StatusCode {
    match err {
        Error::NotFound(_) => StatusCode::NOT_FOUND,
        Error::Forbidden(_) => StatusCode::FORBIDDEN,
        Error::Unauthorized => StatusCode::UNAUTHORIZED,
        Error::Validation(_) => StatusCode::BAD_REQUEST,
        Error::Conflict(_) => StatusCode::CONFLICT,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// A non-leaking, end-user-facing (Portuguese) message keyed by the stable error code.
fn public_message(err: &Error) -> &'static str {
    match err {
        Error::NotFound(_) => "Recurso não encontrado.",
        Error::Forbidden(_) => "Acesso negado.",
        Error::Unauthorized => "Autenticação necessária.",
        Error::Validation(_) => "Requisição inválida.",
        Error::Conflict(_) => "Conflito ao processar a requisição.",
        _ => "Erro interno ao consultar a fila de eventos.",
    }
}

/// `GET /admin/events/health` — queue depth per topic and overall backlog.
async fn queue_health(State(state): State<AppState>) -> impl IntoResponse {
    let queue = EventQueue::new(state.db.clone(), state.clock.clone());
    match queue.queue_depth().await {
        Ok(depths) => (
            StatusCode::OK,
            Json(ApiResponse::ok(QueueHealth::from_depths(depths))),
        ),
        Err(err) => {
            tracing::error!(error = %err, "queue depth query failed");
            (
                status_for(&err),
                Json(ApiResponse::fail(err.code(), public_message(&err))),
            )
        }
    }
}

/// The crate's router, mounted by the gateway under the shared [`AppState`] (ADR-0004).
pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/admin/events/health", get(queue_health))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_mapping_covers_each_variant() {
        assert_eq!(
            status_for(&Error::NotFound("x".into())),
            StatusCode::NOT_FOUND
        );
        assert_eq!(status_for(&Error::Unauthorized), StatusCode::UNAUTHORIZED);
        assert_eq!(
            status_for(&Error::Validation("x".into())),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_for(&Error::Conflict("x".into())),
            StatusCode::CONFLICT
        );
        assert_eq!(
            status_for(&Error::Storage("x".into())),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn health_view_sums_total_pending() {
        let depths = vec![
            QueueDepth {
                topic: "votes".into(),
                pending: 2,
            },
            QueueDepth {
                topic: "proposals".into(),
                pending: 5,
            },
        ];
        let health = QueueHealth::from_depths(depths);
        assert_eq!(health.total_pending, 7);
        assert_eq!(health.topics.len(), 2);
    }

    #[test]
    fn public_message_never_leaks_internal_detail() {
        let err = Error::Storage("secret connection string".into());
        assert!(!public_message(&err).contains("secret"));
    }
}

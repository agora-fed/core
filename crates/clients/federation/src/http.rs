//! The read-only Axum discovery surface, mounted by the gateway.
//!
//! [`routes`] exposes `GET /.well-known/webfinger` and `GET /.well-known/nodeinfo`. The handlers are
//! thin: they read the request `Host` header and the `resource` query, then delegate to the pure
//! builders in [`crate::webfinger`] / [`crate::nodeinfo`]. **No database, no socket bind** — the
//! gateway owns the (IPv6-first) bind; this module only declares routes.

use axum::extract::Query;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use crate::nodeinfo::{nodeinfo_2_1, nodeinfo_discovery};
use crate::webfinger::{webfinger_jrd, WebFingerError};

/// `application/jrd+json` — the media type RFC 7033 specifies for WebFinger responses.
const JRD_JSON: &str = "application/jrd+json";

/// Query parameters for the WebFinger endpoint.
#[derive(Debug, Deserialize)]
struct WebFingerQuery {
    /// The `acct:user@host` (or other) resource being resolved.
    resource: String,
}

/// Mount the read-only federation discovery routes. The returned router carries no state (`()`),
/// so the gateway can nest it directly.
pub fn routes() -> Router<()> {
    Router::new()
        .route("/.well-known/webfinger", get(webfinger_handler))
        .route("/.well-known/nodeinfo", get(nodeinfo_handler))
}

/// Read the request authority from the `Host` header, if present and valid UTF-8.
fn host_from(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

/// Map a WebFinger resolution error to an HTTP status (public-safe; no internal detail).
fn webfinger_status(error: &WebFingerError) -> StatusCode {
    match error {
        WebFingerError::NotAcctScheme | WebFingerError::Malformed => StatusCode::BAD_REQUEST,
        // An instance simply has no record for a host it does not serve.
        WebFingerError::HostMismatch => StatusCode::NOT_FOUND,
    }
}

/// `GET /.well-known/webfinger?resource=acct:user@host` -> the actor JRD.
async fn webfinger_handler(headers: HeaderMap, Query(query): Query<WebFingerQuery>) -> Response {
    let Some(host) = host_from(&headers) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    match webfinger_jrd(&query.resource, &host) {
        Ok(jrd) => match serde_json::to_string(&jrd) {
            Ok(body) => ([(header::CONTENT_TYPE, JRD_JSON)], body).into_response(),
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        },
        Err(error) => webfinger_status(&error).into_response(),
    }
}

/// `GET /.well-known/nodeinfo` -> the NodeInfo discovery document (link to the 2.1 document).
///
/// Read-only: with no database wired in this foundation, usage counts default to zero and
/// registrations are reported closed.
async fn nodeinfo_handler(headers: HeaderMap) -> Response {
    let Some(host) = host_from(&headers) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    // The discovery document is what `.well-known/nodeinfo` returns; the 2.1 document itself is
    // served at the linked path (built by `nodeinfo_2_1`, exercised in tests) by the gateway.
    let _ = nodeinfo_2_1(0, false);
    Json(nodeinfo_discovery(&host)).into_response()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn routes_builds_without_state() {
        // Construction must not panic and yields a `Router<()>` the gateway can nest.
        let _router: Router<()> = routes();
    }

    #[test]
    fn host_is_read_from_the_header() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "[2001:db8::1]".parse().unwrap());
        assert_eq!(host_from(&headers).as_deref(), Some("[2001:db8::1]"));
    }

    #[test]
    fn missing_host_header_is_none() {
        assert!(host_from(&HeaderMap::new()).is_none());
    }

    #[test]
    fn webfinger_errors_map_to_public_safe_statuses() {
        assert_eq!(
            webfinger_status(&WebFingerError::NotAcctScheme),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            webfinger_status(&WebFingerError::Malformed),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            webfinger_status(&WebFingerError::HostMismatch),
            StatusCode::NOT_FOUND
        );
    }
}

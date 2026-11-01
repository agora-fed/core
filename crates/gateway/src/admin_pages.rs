//! Keeps `/admin` pages away from visitors without an administrative role.
//!
//! The pages are static files, so without this check anyone could open the admin UI.
//! The APIs behind them enforce their own authorization; this is not a replacement for it.

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use dsoc_app::AppState;
use uuid::Uuid;

fn is_admin_page(path: &str) -> bool {
    path == "/admin" || path.starts_with("/admin/")
}

fn login_target(path: &str) -> String {
    format!("/entrar/?next={path}")
}

#[derive(Debug, PartialEq, Eq)]
enum Access {
    Anonymous,
    NoRole,
    Allowed,
}

async fn access(state: &AppState, headers: &HeaderMap) -> Access {
    let Some(session) = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|c| crate::cookie_value(c, "dsoc_session"))
        .and_then(|s| Uuid::parse_str(s).ok())
    else {
        return Access::Anonymous;
    };
    let identity = match dsoc_auth::session_identity(&state.db, session).await {
        Ok(Some(identity)) => identity,
        Ok(None) => return Access::Anonymous,
        Err(err) => {
            tracing::error!(?err, "admin page: session lookup failed");
            return Access::Anonymous;
        }
    };
    let (citizen, org) = identity;
    let has_role = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM admin_role_binding
          WHERE org_id = $1 AND citizen_id = $2 AND role IN ('owner', 'admin', 'auditor'))",
    )
    .bind(org)
    .bind(citizen)
    .fetch_one(&state.db)
    .await;
    match has_role {
        Ok(true) => Access::Allowed,
        Ok(false) => Access::NoRole,
        Err(err) => {
            tracing::error!(?err, "admin page: role lookup failed");
            Access::NoRole
        }
    }
}

fn redirect(target: &str) -> Response {
    let location = HeaderValue::from_str(target).unwrap_or(HeaderValue::from_static("/entrar/"));
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, location),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
    )
        .into_response()
}

pub(crate) async fn guard(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_owned();
    if !is_admin_page(&path) {
        return next.run(req).await;
    }
    match access(&state, req.headers()).await {
        Access::Anonymous => redirect(&login_target(&path)),
        Access::NoRole => redirect("/"),
        Access::Allowed => {
            let mut res = next.run(req).await;
            res.headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            res
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_only_admin_pages() {
        assert!(is_admin_page("/admin"));
        assert!(is_admin_page("/admin/"));
        assert!(is_admin_page("/admin/usuarios/"));
        assert!(!is_admin_page("/administracao"));
        assert!(!is_admin_page("/f/admin"));
    }

    #[test]
    fn login_returns_to_the_requested_page() {
        assert_eq!(
            login_target("/admin/modulos/"),
            "/entrar/?next=/admin/modulos/"
        );
    }
}

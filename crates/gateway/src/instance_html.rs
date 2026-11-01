//! Adapts pre-built HTML to the instance: the admin-edited site name and the disabled modules.
//!
//! The site is static (ADR-0009) and crawlers do not run the shell's scripts. Instances with
//! no saved name and every UI-gated module on pass through byte for byte.

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use axum::body::{to_bytes, Body};
use axum::extract::{Request, State};
use axum::http::{header, Method, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use dsoc_app::AppState;
use tokio::sync::RwLock;

const NAME_TTL: Duration = Duration::from_secs(30);
const MAX_HTML_BYTES: usize = 8 * 1024 * 1024;
const NAMED_META: [&str; 3] = [
    "property=\"og:site_name\"",
    "property=\"og:title\"",
    "name=\"twitter:title\"",
];

/// Must match web/src/styles/module-gate.css.
const UI_GATED_MODULES: [&str; 5] = [
    "mandates",
    "proposals",
    "forums",
    "consultations",
    "federation",
];

type CachedName = Option<(Instant, Option<String>)>;
static SITE_NAME: LazyLock<RwLock<CachedName>> = LazyLock::new(|| RwLock::new(None));

pub(crate) fn built_site_name() -> String {
    std::env::var("PUBLIC_SITE_NAME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "AGORA".to_owned())
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

async fn branded_site_name(state: &AppState) -> Option<String> {
    if let Some((at, name)) = SITE_NAME.read().await.as_ref() {
        if at.elapsed() < NAME_TTL {
            return name.clone();
        }
    }
    let name = crate::admin_branding::default_org_site_name(&state.db).await;
    *SITE_NAME.write().await = Some((Instant::now(), name.clone()));
    name
}

pub(crate) async fn invalidate_site_name() {
    *SITE_NAME.write().await = None;
}

/// The name to show right now: the admin's value, else the deployment's.
pub(crate) async fn current_site_name(state: &AppState) -> String {
    branded_site_name(state)
        .await
        .unwrap_or_else(built_site_name)
}

/// Replace `built` with `name` in `<title>` and the social-card metas, leaving the rest intact.
fn rewrite_site_name(html: &str, built: &str, name: &str) -> String {
    let Some(head_end) = html.find("</head>") else {
        return html.to_owned();
    };
    let built = escape_html(built);
    let name = escape_html(name);
    let (head, rest) = html.split_at(head_end);
    let mut out = String::with_capacity(html.len());
    let mut cursor = 0;
    while let Some(start) = next_tag(head, cursor) {
        let closer = if head[start..].starts_with("<title") {
            "</title>"
        } else {
            ">"
        };
        let Some(len) = head[start..].find(closer) else {
            break;
        };
        let end = start + len + closer.len();
        let tag = &head[start..end];
        out.push_str(&head[cursor..start]);
        if tag.starts_with("<title") || NAMED_META.iter().any(|m| tag.contains(m)) {
            out.push_str(&tag.replace(&built, &name));
        } else {
            out.push_str(tag);
        }
        cursor = end;
    }
    out.push_str(&head[cursor..]);
    out.push_str(rest);
    out
}

fn mark_modules_off(html: &str, off: &[&str]) -> String {
    if off.is_empty() {
        return html.to_owned();
    }
    html.replacen(
        "<html",
        &format!("<html data-modules-off=\"{}\"", off.join(" ")),
        1,
    )
}

/// What to change in this instance's pages, or `None` when they are served as built.
pub(crate) struct InstancePatch {
    built: String,
    name: Option<String>,
    modules_off: Vec<&'static str>,
}

impl InstancePatch {
    pub(crate) async fn load(state: &AppState) -> Option<Self> {
        let built = built_site_name();
        let name = branded_site_name(state).await.filter(|n| *n != built);
        let modules_off = crate::module_gate::inactive_for_ui(
            state,
            crate::admin_branding::DEFAULT_ORG_UUID,
            &UI_GATED_MODULES,
        )
        .await;
        (name.is_some() || !modules_off.is_empty()).then_some(Self {
            built,
            name,
            modules_off,
        })
    }

    pub(crate) fn apply(&self, html: &str) -> String {
        let html = match &self.name {
            Some(name) => rewrite_site_name(html, &self.built, name),
            None => html.to_owned(),
        };
        mark_modules_off(&html, &self.modules_off)
    }
}

fn next_tag(head: &str, from: usize) -> Option<usize> {
    let tail = &head[from..];
    [tail.find("<title"), tail.find("<meta ")]
        .into_iter()
        .flatten()
        .min()
        .map(|i| from + i)
}

fn is_page_path(path: &str) -> bool {
    let last = path.rsplit('/').next().unwrap_or("");
    last.is_empty() || last.ends_with(".html") || !last.contains('.')
}

/// Layer for the static site. Conditional headers are dropped because a cached 304 would keep
/// serving the previous name after an admin change.
pub(crate) async fn apply_site_name(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    if req.method() != Method::GET || !is_page_path(req.uri().path()) {
        return next.run(req).await;
    }
    let Some(patch) = InstancePatch::load(&state).await else {
        return next.run(req).await;
    };
    req.headers_mut().remove(header::IF_MODIFIED_SINCE);
    req.headers_mut().remove(header::IF_NONE_MATCH);
    let res = next.run(req).await;

    let is_html = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html"));
    if res.status() != StatusCode::OK || !is_html {
        return res;
    }
    let (mut parts, body) = res.into_parts();
    let Ok(bytes) = to_bytes(body, MAX_HTML_BYTES).await else {
        tracing::error!(
            "static page exceeded {MAX_HTML_BYTES} bytes while applying the instance patch"
        );
        parts.status = StatusCode::INTERNAL_SERVER_ERROR;
        parts.headers.remove(header::CONTENT_LENGTH);
        return Response::from_parts(parts, Body::empty());
    };
    let Ok(html) = std::str::from_utf8(&bytes) else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    let rewritten = patch.apply(html);
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.remove(header::ETAG);
    parts.headers.remove(header::LAST_MODIFIED);
    Response::from_parts(parts, Body::from(rewritten))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "<html><head><title>Sobre · AGORA</title>\
        <meta name=\"description\" content=\"O AGORA e aberto\">\
        <meta property=\"og:title\" content=\"Sobre · AGORA\">\
        <meta property=\"og:site_name\" content=\"AGORA\">\
        <meta name=\"twitter:title\" content=\"AGORA\">\
        </head><body>AGORA</body></html>";

    #[test]
    fn replaces_the_name_in_title_and_social_metas() {
        let out = rewrite_site_name(PAGE, "AGORA", "Coop");
        assert!(out.contains("<title>Sobre · Coop</title>"));
        assert!(out.contains("property=\"og:title\" content=\"Sobre · Coop\""));
        assert!(out.contains("property=\"og:site_name\" content=\"Coop\""));
        assert!(out.contains("name=\"twitter:title\" content=\"Coop\""));
    }

    #[test]
    fn leaves_body_and_unrelated_metas_alone() {
        let out = rewrite_site_name(PAGE, "AGORA", "Coop");
        assert!(out.contains("<body>AGORA</body>"));
        assert!(out.contains("content=\"O AGORA e aberto\""));
    }

    #[test]
    fn escapes_the_admin_value() {
        let out = rewrite_site_name(PAGE, "AGORA", "\"><script>x</script>");
        assert!(!out.contains("<script>"));
        assert!(out.contains("&quot;&gt;&lt;script&gt;"));
    }

    #[test]
    fn page_without_head_is_unchanged() {
        assert_eq!(
            rewrite_site_name("<p>AGORA</p>", "AGORA", "Coop"),
            "<p>AGORA</p>"
        );
    }

    #[test]
    fn unterminated_title_is_preserved() {
        let html = "<head><title>AGORA</head>";
        assert_eq!(rewrite_site_name(html, "AGORA", "Coop"), html);
    }

    #[test]
    fn marks_disabled_modules_on_the_root_only() {
        let out = mark_modules_off(
            "<html lang=\"pt\"><head></head></html>",
            &["mandates", "forums"],
        );
        assert_eq!(
            out,
            "<html data-modules-off=\"mandates forums\" lang=\"pt\"><head></head></html>"
        );
    }

    #[test]
    fn nothing_disabled_leaves_the_page_unchanged() {
        assert_eq!(mark_modules_off("<html></html>", &[]), "<html></html>");
    }

    #[test]
    fn every_gated_module_has_a_css_rule_and_a_catalog_entry() {
        let css = include_str!("../../../web/src/styles/module-gate.css");
        for id in UI_GATED_MODULES {
            assert!(css.contains(&format!("[data-requires~='{id}']")), "{id}");
            assert!(crate::module_catalog::find(id).is_some(), "{id}");
        }
    }

    #[test]
    fn only_pages_are_candidates() {
        assert!(is_page_path("/"));
        assert!(is_page_path("/sobre/"));
        assert!(is_page_path("/sobre"));
        assert!(is_page_path("/sobre/index.html"));
        assert!(!is_page_path("/_astro/app.js"));
        assert!(!is_page_path("/og-image.png"));
    }
}

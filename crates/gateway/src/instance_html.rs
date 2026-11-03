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

/// First path segment of static pages that belong to a module.
const MODULE_ROUTES: [(&str, &str); 19] = [
    ("politicos", "mandates"),
    ("partidos", "mandates"),
    ("eleicoes", "mandates"),
    ("servicos", "mandates"),
    ("campanha", "mandates"),
    ("painel-mandato", "mandates"),
    ("meu-grupo", "mandates"),
    ("grupo", "mandates"),
    ("municipio", "mandates"),
    ("imprensa", "mandates"),
    ("propostas", "proposals"),
    ("propor", "proposals"),
    ("consultas", "consultations"),
    ("consulta", "consultations"),
    ("feed", "federation"),
    ("explorar", "federation"),
    ("rede", "federation"),
    ("publicacao", "federation"),
    ("tag", "federation"),
];
const UNAVAILABLE_PAGE: &str = "/modulo-indisponivel/";

/// First path segment of static pages that belong to a module that is off by default.
const OPTIONAL_MODULE_ROUTES: &[(&str, &str)] = &[("grupos", "groups"), ("reunioes", "meetings")];

/// Off by default, so listed only when on. Must match web/src/styles/module-gate.css.
const UI_OPTIONAL_MODULES: &[&str] = &["groups", "meetings"];

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

/// The page to serve instead of the requested one, with its status.
fn route_override(path: &str, off: &[&str], on: &[&str]) -> Option<(&'static str, StatusCode)> {
    let trimmed = path.trim_matches('/');
    if off.contains(&"mandates") {
        match trimmed {
            "" => return Some(("/inicio-comunidade/", StatusCode::OK)),
            "sobre" => return Some(("/sobre-comunidade/", StatusCode::OK)),
            _ => {}
        }
    }
    let first = trimmed.split('/').next().unwrap_or("");
    let gated_off = MODULE_ROUTES
        .iter()
        .any(|(route, module)| *route == first && off.contains(module));
    let optional_off = OPTIONAL_MODULE_ROUTES
        .iter()
        .any(|(route, module)| *route == first && !on.contains(module));
    (gated_off || optional_off).then_some((UNAVAILABLE_PAGE, StatusCode::NOT_FOUND))
}

fn mark_modules(html: &str, attribute: &str, ids: &[&str]) -> String {
    if ids.is_empty() {
        return html.to_owned();
    }
    html.replacen(
        "<html",
        &format!("<html {attribute}=\"{}\"", ids.join(" ")),
        1,
    )
}

/// What to change in this instance's pages.
pub(crate) struct InstancePatch {
    built: String,
    name: Option<String>,
    modules_off: Vec<&'static str>,
    modules_on: Vec<&'static str>,
}

impl InstancePatch {
    pub(crate) async fn load(state: &AppState) -> Self {
        let built = built_site_name();
        let name = branded_site_name(state).await.filter(|n| *n != built);
        let org = crate::admin_branding::DEFAULT_ORG_UUID;
        let modules_off = crate::module_gate::inactive_for_ui(state, org, &UI_GATED_MODULES).await;
        let modules_on = crate::module_gate::active_for_ui(state, org, UI_OPTIONAL_MODULES).await;
        Self {
            built,
            name,
            modules_off,
            modules_on,
        }
    }

    /// True when pages can be served as built, keeping their validators.
    fn is_noop(&self) -> bool {
        self.name.is_none() && self.modules_off.is_empty() && self.modules_on.is_empty()
    }

    pub(crate) fn apply(&self, html: &str) -> String {
        let html = match &self.name {
            Some(name) => rewrite_site_name(html, &self.built, name),
            None => html.to_owned(),
        };
        let html = mark_modules(&html, "data-modules-off", &self.modules_off);
        mark_modules(&html, "data-modules-on", &self.modules_on)
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
    let patch = InstancePatch::load(&state).await;
    let page = route_override(req.uri().path(), &patch.modules_off, &patch.modules_on);
    if page.is_none() && patch.is_noop() {
        return next.run(req).await;
    }
    if let Some((target, _)) = page {
        match target.parse() {
            Ok(uri) => *req.uri_mut() = uri,
            Err(err) => tracing::error!(?err, target, "invalid page override"),
        }
    }
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
    if let Some((_, status)) = page {
        parts.status = status;
    }
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
    fn home_and_about_swap_only_when_mandates_is_off() {
        assert_eq!(
            route_override("/", &["mandates"], &[]),
            Some(("/inicio-comunidade/", StatusCode::OK))
        );
        assert_eq!(
            route_override("/sobre/", &["mandates"], &[]),
            Some(("/sobre-comunidade/", StatusCode::OK))
        );
        assert_eq!(route_override("/", &["forums"], &[]), None);
    }

    #[test]
    fn routes_of_disabled_modules_are_unavailable() {
        let expected = Some((UNAVAILABLE_PAGE, StatusCode::NOT_FOUND));
        assert_eq!(
            route_override("/politicos/abc/", &["mandates"], &[]),
            expected
        );
        assert_eq!(route_override("/propor", &["proposals"], &[]), expected);
    }

    #[test]
    fn route_match_uses_whole_segments() {
        assert_eq!(route_override("/politicos-x/", &["mandates"], &[]), None);
        assert_eq!(route_override("/politicos/", &["forums"], &[]), None);
    }

    #[test]
    fn optional_routes_are_unavailable_unless_on() {
        let expected = Some((UNAVAILABLE_PAGE, StatusCode::NOT_FOUND));
        assert_eq!(route_override("/grupos/", &[], &[]), expected);
        assert_eq!(route_override("/grupos/", &[], &["groups"]), None);
        assert_eq!(route_override("/reunioes/", &[], &[]), expected);
        assert_eq!(route_override("/reunioes/", &[], &["meetings"]), None);
    }

    #[test]
    fn default_instance_is_served_as_built() {
        let patch = InstancePatch {
            built: "AGORA".to_owned(),
            name: None,
            modules_off: Vec::new(),
            modules_on: Vec::new(),
        };
        assert!(patch.is_noop());
    }

    #[test]
    fn module_routes_point_at_listed_modules() {
        for (_, module) in MODULE_ROUTES {
            assert!(UI_GATED_MODULES.contains(&module), "{module}");
        }
        for (_, module) in OPTIONAL_MODULE_ROUTES {
            assert!(UI_OPTIONAL_MODULES.contains(module), "{module}");
        }
    }

    #[test]
    fn marks_disabled_modules_on_the_root_only() {
        let out = mark_modules(
            "<html lang=\"pt\"><head></head></html>",
            "data-modules-off",
            &["mandates", "forums"],
        );
        assert_eq!(
            out,
            "<html data-modules-off=\"mandates forums\" lang=\"pt\"><head></head></html>"
        );
    }

    #[test]
    fn nothing_disabled_leaves_the_page_unchanged() {
        assert_eq!(
            mark_modules("<html></html>", "data-modules-off", &[]),
            "<html></html>"
        );
    }

    #[test]
    fn every_gated_module_has_a_css_rule_and_a_catalog_entry() {
        let css = include_str!("../../../web/src/styles/module-gate.css");
        for id in UI_GATED_MODULES {
            let rule = format!("html[data-modules-off~='{id}'] [data-requires~='{id}']");
            assert!(css.contains(&rule), "{id}");
            let module = crate::module_catalog::find(id).expect(id);
            assert!(module.default_enabled, "{id} must be on by default");
        }
        for id in UI_OPTIONAL_MODULES {
            let rule = format!("html:not([data-modules-on~='{id}']) [data-requires~='{id}']");
            assert!(css.contains(&rule), "{id}");
            let module = crate::module_catalog::find(id).expect(id);
            assert!(!module.default_enabled, "{id} must be off by default");
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

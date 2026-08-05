//! # Runtime branding — admin-editable visual identity (migration 0674).
//!
//! Odoo-style installation branding: logo, site name, tagline and the semantic
//! color tokens live in the DATABASE, edited from the admin console and applied
//! by the web shell at runtime. Nothing here is a build artifact.
//!
//! Routes (merged into the gateway's `/api/v1` group via [`routes`]):
//! * `GET /branding`       — PUBLIC. The active branding for the caller's org
//!   (empty defaults when unset — the shell keeps the shipped theme).
//! * `GET /admin/branding` — the same payload, behind the admin gate (panel load).
//! * `PUT /admin/branding` — upsert. Admin-gated, validated field by field.
//!
//! Design notes:
//! * `colors` only accepts the semantic tokens in [`ALLOWED_COLOR_TOKENS`] with
//!   `#rgb`/`#rrggbb`/`#rrggbbaa` values — an admin can restyle, never inject CSS.
//! * Runtime-unchecked `sqlx` (same policy as `parties.rs`): the committed
//!   `.sqlx/` offline cache stays untouched on DB-less build hosts.
//! * Public endpoint degrades to an all-`null` payload — never an error — so a
//!   fresh installation renders the shipped theme untouched.

use axum::extract::{Json, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::PgPool;
use uuid::Uuid;

const DEFAULT_ORG_UUID: Uuid = uuid::uuid!("11111111-1111-1111-1111-111111111111");

/// Semantic color tokens an admin may override (the web shell maps each to the
/// CSS custom property of the same name). Extending this list is a code change
/// on purpose: it is the contract between the admin UI and the design system
/// (web/src/styles/tokens.css semantic layer).
pub const ALLOWED_COLOR_TOKENS: [&str; 23] = [
    // Brand
    "accent",
    "accent-strong",
    "accent-soft",
    "accent-contrast",
    // Surfaces
    "surface-0",
    "surface-1",
    "surface-2",
    "surface-3",
    "surface-inverse",
    // Text
    "text-1",
    "text-2",
    "text-3",
    "text-inverse",
    // Borders
    "border-subtle",
    "border-strong",
    // State
    "danger",
    "danger-soft",
    "warning",
    "warning-soft",
    "info",
    "info-soft",
    "success",
    "success-soft",
];

const MAX_SITE_NAME: usize = 80;
const MAX_TAGLINE: usize = 200;
const MAX_URL: usize = 500;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/branding", get(public_branding))
        .route("/admin/branding", get(admin_branding).put(put_branding))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

/// The branding payload (public and admin views are identical: nothing here is
/// sensitive — it is by definition what every visitor sees).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BrandingDto {
    pub site_name: Option<String>,
    pub tagline: Option<String>,
    pub logo_url: Option<String>,
    pub favicon_url: Option<String>,
    /// `{semantic-token: #hex}` — only [`ALLOWED_COLOR_TOKENS`] survive validation.
    pub colors: Map<String, Value>,
}

/// PUT body. Every field optional; `None` clears the stored value (full-state
/// upsert, mirroring what the panel holds on screen).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct BrandingInput {
    pub site_name: Option<String>,
    pub tagline: Option<String>,
    pub logo_url: Option<String>,
    pub favicon_url: Option<String>,
    #[serde(default)]
    pub colors: Map<String, Value>,
}

// ---------------------------------------------------------------------------
// Helpers (same conventions as admin_content.rs)
// ---------------------------------------------------------------------------

fn caller_org(headers: &HeaderMap) -> Uuid {
    headers
        .get("x-dsoc-org-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_ORG_UUID)
}

fn fail(status: StatusCode, code: &str, msg: &str) -> Response {
    (status, Json(ApiResponse::<()>::fail(code, msg))).into_response()
}

/// Admin gate: `admin_role_binding` role owner/admin (same criterion as
/// `admin_ext`/`admin_content`). Err carries the ready-made response.
/// Org-scoped admin gate — delegates to the single implementation in
/// [`crate::authz_ext::require_org_admin`] (issue #8).
async fn require_admin(db: &PgPool, headers: &HeaderMap) -> Result<Uuid, Response> {
    crate::authz_ext::require_org_admin(db, headers)
        .await
        .map(|a| a.citizen)
}

/// `#rgb`, `#rrggbb` or `#rrggbbaa` (case-insensitive). No other CSS accepted.
fn is_hex_color(s: &str) -> bool {
    let Some(hex) = s.strip_prefix('#') else {
        return false;
    };
    matches!(hex.len(), 3 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit())
}

/// Accept absolute http(s) URLs or site-relative paths (`/media/...`).
fn is_safe_url(s: &str) -> bool {
    s.len() <= MAX_URL
        && (s.starts_with("https://") || s.starts_with("http://") || s.starts_with('/'))
}

/// Validate the color map against the token allowlist. Returns the cleaned map
/// or the offending key.
fn validate_colors(colors: &Map<String, Value>) -> Result<Map<String, Value>, String> {
    let mut clean = Map::new();
    for (token, value) in colors {
        if !ALLOWED_COLOR_TOKENS.contains(&token.as_str()) {
            return Err(format!("unknown color token: {token}"));
        }
        let Some(hex) = value.as_str().filter(|v| is_hex_color(v)) else {
            return Err(format!("invalid color for {token}: expected #hex"));
        };
        clean.insert(token.clone(), Value::String(hex.to_owned()));
    }
    Ok(clean)
}

async fn load(db: &PgPool, org: Uuid) -> BrandingDto {
    let row: Option<(
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Value,
    )> = sqlx::query_as(
        r"SELECT site_name, tagline, logo_url, favicon_url, colors
          FROM org_branding WHERE org_id = $1",
    )
    .bind(org)
    .fetch_optional(db)
    .await
    .ok()
    .flatten();
    match row {
        Some((site_name, tagline, logo_url, favicon_url, colors)) => BrandingDto {
            site_name,
            tagline,
            logo_url,
            favicon_url,
            colors: colors.as_object().cloned().unwrap_or_default(),
        },
        None => BrandingDto::default(),
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /branding` — public read; empty defaults when never configured.
async fn public_branding(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let dto = load(&state.db, caller_org(&headers)).await;
    (StatusCode::OK, Json(ApiResponse::ok(dto))).into_response()
}

/// `GET /admin/branding` — panel load (same payload, admin-gated).
async fn admin_branding(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_admin(&state.db, &headers).await {
        return resp;
    }
    let dto = load(&state.db, caller_org(&headers)).await;
    (StatusCode::OK, Json(ApiResponse::ok(dto))).into_response()
}

/// `PUT /admin/branding` — validated full-state upsert.
async fn put_branding(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<BrandingInput>,
) -> Response {
    let citizen = match require_admin(&state.db, &headers).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    if input
        .site_name
        .as_deref()
        .is_some_and(|s| s.trim().is_empty() || s.len() > MAX_SITE_NAME)
    {
        return fail(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "Nome do site inválido.",
        );
    }
    if input
        .tagline
        .as_deref()
        .is_some_and(|s| s.len() > MAX_TAGLINE)
    {
        return fail(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "Slogan longo demais.",
        );
    }
    for url in [input.logo_url.as_deref(), input.favicon_url.as_deref()]
        .into_iter()
        .flatten()
    {
        if !is_safe_url(url) {
            return fail(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                "URL de imagem inválida (http(s) ou caminho /...).",
            );
        }
    }
    let colors = match validate_colors(&input.colors) {
        Ok(c) => c,
        Err(err) => return fail(StatusCode::UNPROCESSABLE_ENTITY, "validation", &err),
    };

    let org = caller_org(&headers);
    let result = sqlx::query(
        r"INSERT INTO org_branding
              (org_id, site_name, tagline, logo_url, favicon_url, colors, updated_by, updated_at)
          VALUES ($1, $2, $3, $4, $5, $6, $7, now())
          ON CONFLICT (org_id) DO UPDATE SET
              site_name   = EXCLUDED.site_name,
              tagline     = EXCLUDED.tagline,
              logo_url    = EXCLUDED.logo_url,
              favicon_url = EXCLUDED.favicon_url,
              colors      = EXCLUDED.colors,
              updated_by  = EXCLUDED.updated_by,
              updated_at  = now()",
    )
    .bind(org)
    .bind(input.site_name.as_deref().map(str::trim))
    .bind(input.tagline.as_deref().map(str::trim))
    .bind(input.logo_url.as_deref())
    .bind(input.favicon_url.as_deref())
    .bind(Value::Object(colors))
    .bind(citizen)
    .execute(&state.db)
    .await;
    match result {
        Ok(_) => {
            let dto = load(&state.db, org).await;
            (StatusCode::OK, Json(ApiResponse::ok(dto))).into_response()
        }
        Err(err) => {
            tracing::error!(?err, "branding: upsert failed");
            fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage",
                "Erro interno.",
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_color_validation_accepts_rgb_rrggbb_rrggbbaa_only() {
        assert!(is_hex_color("#15803d"));
        assert!(is_hex_color("#fff"));
        assert!(is_hex_color("#15803dCC"));
        assert!(!is_hex_color("15803d"));
        assert!(!is_hex_color("#15803")); // 5 digits
        assert!(!is_hex_color("#gggggg"));
        assert!(!is_hex_color("red"));
        assert!(!is_hex_color("#fff; background:url(x)"));
    }

    #[test]
    fn color_map_rejects_unknown_tokens_and_non_hex_values() {
        let mut colors = Map::new();
        colors.insert("accent".into(), Value::String("#22c55e".into()));
        assert!(validate_colors(&colors).is_ok());

        colors.insert("background".into(), Value::String("#000".into()));
        assert!(validate_colors(&colors).is_err()); // token not allowlisted

        let mut evil = Map::new();
        evil.insert(
            "accent".into(),
            Value::String("url(javascript:alert(1))".into()),
        );
        assert!(validate_colors(&evil).is_err());
    }

    #[test]
    fn url_validation_accepts_https_and_relative_paths_only() {
        assert!(is_safe_url("https://cdn.example.org/logo.png"));
        assert!(is_safe_url("/media/logo.png"));
        assert!(!is_safe_url("javascript:alert(1)"));
        assert!(!is_safe_url("data:image/png;base64,xxxx"));
    }
}

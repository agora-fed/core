-- 0674 — org_branding: runtime, admin-editable visual identity (Odoo-style).
--
-- Branding is INSTALLATION STATE, not build artifact: logo, name, tagline and
-- the semantic color tokens are edited in the admin UI and applied by the web
-- shell at runtime (CSS custom properties). One row per org; absent row means
-- "use the theme defaults shipped with the build".
--
-- `colors` is a jsonb map {semantic-token: #hex} validated at the API layer
-- against an allowlist (accent, accent-strong, accent-soft, accent-contrast)
-- so an admin can never inject arbitrary CSS.

CREATE TABLE org_branding (
    org_id      uuid PRIMARY KEY REFERENCES org(id),
    site_name   text,
    tagline     text,
    logo_url    text,
    favicon_url text,
    colors      jsonb NOT NULL DEFAULT '{}'::jsonb,
    updated_by  uuid REFERENCES citizen(id),
    updated_at  timestamptz NOT NULL DEFAULT now()
);

COMMENT ON TABLE org_branding IS
    'gateway/admin: runtime visual identity edited in the admin UI. One row per org; validated color-token allowlist in the API layer.';
COMMENT ON COLUMN org_branding.colors IS
    'jsonb map {semantic-token: #hex}; allowlisted tokens only (accent*, ...).';

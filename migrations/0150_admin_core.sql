-- 0150_admin_core — owned by `dsoc-admin` (Tier 1, migration range 0150).
-- System & organization administration: an org extension record, administrative
-- role bindings, and per-org feature flags. Explicit, auditable SQL (PLAN.md
-- principle 3). Cross-crate FKs target ONLY core identity tables (`org`, `citizen`)
-- per migrations/REGISTRY.md. No IPv4 anywhere (PLAN.md principle 4).

-- ---------------------------------------------------------------------------
-- admin_org — 1:1 administrative extension of a baseline `org` (core-owned).
-- The baseline row is created at registration; admin links and governs it.
-- ---------------------------------------------------------------------------
CREATE TABLE admin_org (
    org_id      uuid PRIMARY KEY REFERENCES org(id),
    -- Whether the organization is administratively active (soft enable/disable).
    is_active   boolean NOT NULL DEFAULT true,
    created_at  timestamptz NOT NULL
);

-- ---------------------------------------------------------------------------
-- admin_role_binding — grants a citizen an administrative role in an org.
-- ---------------------------------------------------------------------------
CREATE TABLE admin_role_binding (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES org(id),
    citizen_id  uuid NOT NULL REFERENCES citizen(id),
    -- Mirrors `dsoc_admin::domain::AdminRole`; kept in sync by a CHECK.
    role        text NOT NULL CHECK (role IN ('owner', 'admin', 'auditor')),
    created_at  timestamptz NOT NULL,
    -- A citizen holds a given role in a given org at most once (idempotent grants).
    UNIQUE (org_id, citizen_id, role)
);
-- Keyset pagination support: list a single org's bindings ordered by id.
CREATE INDEX admin_role_binding_org_idx ON admin_role_binding (org_id, id);

-- ---------------------------------------------------------------------------
-- admin_feature_flag — per-org feature toggle. Toggling is idempotent (UNIQUE
-- key) and audited via created_at/updated_at written from the injected Clock.
-- ---------------------------------------------------------------------------
CREATE TABLE admin_feature_flag (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES org(id),
    key         text NOT NULL,
    enabled     boolean NOT NULL DEFAULT false,
    created_at  timestamptz NOT NULL,
    updated_at  timestamptz NOT NULL,
    -- One row per (org, key): the upsert target that makes toggling idempotent.
    UNIQUE (org_id, key)
);
-- Keyset pagination support: list a single org's flags ordered by id.
CREATE INDEX admin_feature_flag_org_idx ON admin_feature_flag (org_id, id);

COMMENT ON TABLE admin_org IS 'Administrative extension of core-owned org; see crates/platform/admin';

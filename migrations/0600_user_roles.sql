-- Migration 0600 — Mastodon-style flexible roles (R0.2 / issue #39, ADR-0011).
--
-- Reservation: 0600–0649 is the CORE range (base/identity) defined in ADR-0011 (R0.8),
-- fixing the root cause of the historical leakage of `citizen` migrations.
--
-- Replaces the hard CHECK of `admin_role_binding` (owner|admin|auditor) with CONFIGURABLE
-- roles: each org has N roles, each role carries an OPEN set of
-- `module.action` permission keys (permissions text[]); the checkbox matrix of
-- /admin/papeis (R4) is built from the active modules. Hierarchy by `position` (Mastodon:
-- you only manage a role of a lower position). `administrator` in the list bypasses everything.
--
-- `admin_role_binding` is NOT dropped here — the interim gates of the security queue
-- (0.59.2/0.59.3) still read it; it goes when RequirePermission migrates (R0.3).
--
-- Idempotente: rerun-safe.

BEGIN;

-- ---------------------------------------------------------------------------
-- user_role — a configurable role within an org.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS user_role (
    id           uuid PRIMARY KEY,
    org_id       uuid NOT NULL REFERENCES org(id),
    name         text NOT NULL,
    color        text,                              -- hex opcional pro badge (#RRGGBB)
    position     integer NOT NULL DEFAULT 0,        -- hierarquia; maior manda
    permissions  text[] NOT NULL DEFAULT '{}',      -- chaves modulo.acao (aberto por módulo)
    highlighted  boolean NOT NULL DEFAULT false,    -- exibe badge no perfil público
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    -- Name unique per org (the id distinguishes; the name is the human label).
    UNIQUE (org_id, name)
);
CREATE INDEX IF NOT EXISTS user_role_org_position_idx
    ON user_role (org_id, position DESC);
ALTER TABLE user_role OWNER TO dsoc;

-- ---------------------------------------------------------------------------
-- citizen_role_binding — grants a role to a citizen in an org (N roles/citizen).
-- The Base role (position 0) is NOT bound: the permission resolver always
-- aggregates it for every caller of the org (equivalent to Mastodon's "everyone" role id -99).
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS citizen_role_binding (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES org(id),
    citizen_id  uuid NOT NULL REFERENCES citizen(id),
    role_id     uuid NOT NULL REFERENCES user_role(id) ON DELETE CASCADE,
    created_by  uuid REFERENCES citizen(id),
    created_at  timestamptz NOT NULL DEFAULT now(),
    UNIQUE (org_id, citizen_id, role_id)
);
CREATE INDEX IF NOT EXISTS citizen_role_binding_citizen_idx
    ON citizen_role_binding (org_id, citizen_id);
ALTER TABLE citizen_role_binding OWNER TO dsoc;

-- ---------------------------------------------------------------------------
-- Seeds: 5 default roles per existing org. Idempotent via ON CONFLICT (org,name).
-- The keys mirror the ADR-0011 catalog; `administrator` bypasses everything, so the
-- Owner only needs that one.
-- ---------------------------------------------------------------------------
INSERT INTO user_role (id, org_id, name, color, position, permissions, highlighted)
SELECT gen_random_uuid(), o.id, 'Proprietário', '#7c3aed', 1000,
       ARRAY['administrator']::text[], true
  FROM org o
ON CONFLICT (org_id, name) DO NOTHING;

INSERT INTO user_role (id, org_id, name, color, position, permissions, highlighted)
SELECT gen_random_uuid(), o.id, 'Administrador', '#dc2626', 100,
       ARRAY[
         'view_dashboard','view_audit_log','roles.manage','orgs.manage','flags.manage',
         'users.view','users.manage','users.access','reports.manage','content.moderate',
         'forums.moderate','federation.manage','announcements.manage',
         'email_templates.manage','webhooks.manage','invites.manage'
       ]::text[], true
  FROM org o
ON CONFLICT (org_id, name) DO NOTHING;

INSERT INTO user_role (id, org_id, name, color, position, permissions, highlighted)
SELECT gen_random_uuid(), o.id, 'Moderador', '#2563eb', 10,
       ARRAY[
         'view_dashboard','view_audit_log','reports.manage','content.moderate',
         'forums.moderate','users.view'
       ]::text[], true
  FROM org o
ON CONFLICT (org_id, name) DO NOTHING;

INSERT INTO user_role (id, org_id, name, color, position, permissions, highlighted)
SELECT gen_random_uuid(), o.id, 'Auditoria', '#0891b2', 5,
       ARRAY['view_dashboard','view_audit_log']::text[], false
  FROM org o
ON CONFLICT (org_id, name) DO NOTHING;

INSERT INTO user_role (id, org_id, name, color, position, permissions, highlighted)
SELECT gen_random_uuid(), o.id, 'Base', NULL, 0,
       ARRAY[]::text[], false
  FROM org o
ON CONFLICT (org_id, name) DO NOTHING;

-- ---------------------------------------------------------------------------
-- Compat: each admin_role_binding row becomes a citizen_role_binding for the
-- equivalent role (owner→Proprietário, admin→Administrador, auditor→Auditoria).
-- ---------------------------------------------------------------------------
INSERT INTO citizen_role_binding (id, org_id, citizen_id, role_id, created_by, created_at)
SELECT gen_random_uuid(), b.org_id, b.citizen_id, r.id, NULL, b.created_at
  FROM admin_role_binding b
  JOIN user_role r
    ON r.org_id = b.org_id
   AND r.name = CASE b.role
                  WHEN 'owner'   THEN 'Proprietário'
                  WHEN 'admin'   THEN 'Administrador'
                  WHEN 'auditor' THEN 'Auditoria'
                END
ON CONFLICT (org_id, citizen_id, role_id) DO NOTHING;

COMMENT ON TABLE user_role IS
    '0600: papéis configuráveis por org (ADR-0011); permissions text[] = chaves modulo.acao.';
COMMENT ON TABLE citizen_role_binding IS
    '0600: concessão de papel a cidadão; papel Base (pos 0) é implícito, não bindado.';

COMMIT;

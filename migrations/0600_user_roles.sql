-- Migration 0600 — Papéis flexíveis estilo Mastodon (R0.2 / issue #39, ADR-0011).
--
-- Reserva: 0600–0649 é a faixa CORE (base/identidade) definida no ADR-0011 (R0.8),
-- corrigindo a causa-raiz do vazamento histórico de migrações de `citizen`.
--
-- Substitui o CHECK duro de `admin_role_binding` (owner|admin|auditor) por papéis
-- CONFIGURÁVEIS: cada org tem N papéis, cada papel carrega um conjunto ABERTO de
-- chaves de permissão `modulo.acao` (permissions text[]); a matriz de checkboxes do
-- /admin/papeis (R4) se monta dos módulos ativos. Hierarquia por `position` (Mastodon:
-- só gerencia papel de posição menor). `administrator` na lista bypassa tudo.
--
-- `admin_role_binding` NÃO é dropado aqui — os gates interinos da fila de segurança
-- (0.59.2/0.59.3) ainda o leem; sai quando o RequirePermission migrar (R0.3).
--
-- Idempotente: rerun-safe.

BEGIN;

-- ---------------------------------------------------------------------------
-- user_role — um papel configurável dentro de uma org.
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
    -- Nome único por org (o id distingue; o nome é a etiqueta humana).
    UNIQUE (org_id, name)
);
CREATE INDEX IF NOT EXISTS user_role_org_position_idx
    ON user_role (org_id, position DESC);
ALTER TABLE user_role OWNER TO dsoc;

-- ---------------------------------------------------------------------------
-- citizen_role_binding — concede um papel a um cidadão numa org (N papéis/cidadão).
-- O papel Base (position 0) NÃO é bindado: o resolver de permissões sempre o
-- agrega pra todo caller da org (equivale ao papel "everyone" id -99 do Mastodon).
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
-- Seeds: 5 papéis padrão por org existente. Idempotente via ON CONFLICT (org,name).
-- As chaves espelham o catálogo do ADR-0011; `administrator` bypassa tudo, então o
-- Proprietário só precisa dela.
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
-- Compat: cada linha de admin_role_binding vira um citizen_role_binding pro
-- papel equivalente (owner→Proprietário, admin→Administrador, auditor→Auditoria).
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

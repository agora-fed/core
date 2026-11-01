-- AGORA cooperative profile seed
-- Run AFTER migrations to configure for cooperative governance (not political tracking).
-- Usage: psql -d agora -f seed-cooperative.sql
--
-- Takes over the gateway's default organization (requests without an org context
-- resolve to it, so module flags and branding only apply there) and configures modules for cooperatives:
-- disables political tracking, enables governance tools.

-- Create the default organization
INSERT INTO org (id, slug, name, created_at)
VALUES (
    '11111111-1111-1111-1111-111111111111'::uuid,
    'organica',
    'Orgânica Social',
    now()
)
ON CONFLICT (id) DO UPDATE SET slug = EXCLUDED.slug, name = EXCLUDED.name;

-- Admin extension
INSERT INTO admin_org (org_id, is_active, created_at)
VALUES ('11111111-1111-1111-1111-111111111111', true, now())
ON CONFLICT (org_id) DO NOTHING;

-- Disable political modules
INSERT INTO admin_feature_flag (id, org_id, key, enabled, created_at, updated_at)
VALUES
    (gen_random_uuid(), '11111111-1111-1111-1111-111111111111', 'module.mandates',       false, now(), now()),
    (gen_random_uuid(), '11111111-1111-1111-1111-111111111111', 'module.consequence',    false, now(), now()),
    (gen_random_uuid(), '11111111-1111-1111-1111-111111111111', 'module.scorecard',      false, now(), now()),
    (gen_random_uuid(), '11111111-1111-1111-1111-111111111111', 'module.accountability', false, now(), now()),
    (gen_random_uuid(), '11111111-1111-1111-1111-111111111111', 'module.proposals',      false, now(), now())
ON CONFLICT (org_id, key) DO UPDATE SET enabled = EXCLUDED.enabled, updated_at = now();

-- Enable governance modules (off by default in Pindorama)
INSERT INTO admin_feature_flag (id, org_id, key, enabled, created_at, updated_at)
VALUES
    (gen_random_uuid(), '11111111-1111-1111-1111-111111111111', 'module.forums',        true, now(), now()),
    (gen_random_uuid(), '11111111-1111-1111-1111-111111111111', 'module.budgets',       true, now(), now()),
    (gen_random_uuid(), '11111111-1111-1111-1111-111111111111', 'module.surveys',       true, now(), now()),
    (gen_random_uuid(), '11111111-1111-1111-1111-111111111111', 'module.meetings',      true, now(), now()),
    (gen_random_uuid(), '11111111-1111-1111-1111-111111111111', 'module.assemblies',    true, now(), now()),
    (gen_random_uuid(), '11111111-1111-1111-1111-111111111111', 'module.processes',     true, now(), now()),
    (gen_random_uuid(), '11111111-1111-1111-1111-111111111111', 'module.consultations', true, now(), now()),
    (gen_random_uuid(), '11111111-1111-1111-1111-111111111111', 'module.federation',    true, now(), now()),
    (gen_random_uuid(), '11111111-1111-1111-1111-111111111111', 'module.comments',      true, now(), now()),
    (gen_random_uuid(), '11111111-1111-1111-1111-111111111111', 'module.votes',         true, now(), now())
ON CONFLICT (org_id, key) DO UPDATE SET enabled = EXCLUDED.enabled, updated_at = now();

-- Replace the ministry-based interest areas, keeping any already chosen by someone.
DELETE FROM interest_area a
 WHERE NOT EXISTS (SELECT 1 FROM citizen_interest c WHERE c.area_slug = a.slug);
INSERT INTO interest_area (slug, name, ministry, position) VALUES
    ('governanca', 'Assembleias e governança', NULL, 1),
    ('financas', 'Finanças e orçamento', NULL, 2),
    ('producao', 'Produção e trabalho', NULL, 3),
    ('educacao-cooperativa', 'Educação cooperativista', NULL, 4),
    ('comunicacao', 'Comunicação', NULL, 5),
    ('comunidade', 'Comunidade e território', NULL, 6),
    ('meio-ambiente', 'Meio ambiente', NULL, 7),
    ('tecnologia', 'Tecnologia', NULL, 8)
ON CONFLICT (slug) DO NOTHING;

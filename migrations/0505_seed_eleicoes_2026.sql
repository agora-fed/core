-- Migration 0505 — seed das 4 elections estruturais para 2026.
--
-- Não é migração de schema (nada de DDL), é seed idempotente. Cria as 4
-- linhas em `election` que o comparador `/eleicoes/2026` precisa pra
-- listar os pleitos:
--
--   1. Federal — 1º turno (04/10/2026): Presidente + Senador +
--      Deputado Federal
--   2. Federal — 2º turno (25/10/2026): Presidente (se houver)
--   3. Estadual — 1º turno (04/10/2026): Governador + Deputado Estadual
--   4. Estadual — 2º turno (25/10/2026): Governador (se houver)
--
-- Datas conforme calendário TSE oficial. registration_deadline = 15/08/2026
-- (prazo final registro de candidatura, Res. TSE 23.735/2024).
--
-- Idempotente via UNIQUE(org_id, year, round, sphere).

BEGIN;

-- Self-containment guard: the default org this seed points at was historically
-- created by hand, which made the migration chain unreproducible on a fresh
-- database (CI). Idempotent — existing prod/dev rows are untouched.
INSERT INTO org (id, slug, name, created_at)
VALUES ('11111111-1111-1111-1111-111111111111'::uuid, 'brasil', 'DemocraciaBR', now())
ON CONFLICT (id) DO NOTHING;

INSERT INTO election (id, org_id, year, round, sphere,
                      election_day, registration_deadline, created_at)
VALUES
    -- Federal — 1º turno
    ('a0000001-0000-4000-8000-000020261001'::uuid,
     '11111111-1111-1111-1111-111111111111'::uuid,
     2026, 1, 'federal',
     DATE '2026-10-04', DATE '2026-08-15', now()),
    -- Federal — 2º turno
    ('a0000002-0000-4000-8000-000020261002'::uuid,
     '11111111-1111-1111-1111-111111111111'::uuid,
     2026, 2, 'federal',
     DATE '2026-10-25', DATE '2026-08-15', now()),
    -- Estadual — 1º turno
    ('a0000003-0000-4000-8000-000020261003'::uuid,
     '11111111-1111-1111-1111-111111111111'::uuid,
     2026, 1, 'estadual',
     DATE '2026-10-04', DATE '2026-08-15', now()),
    -- Estadual — 2º turno
    ('a0000004-0000-4000-8000-000020261004'::uuid,
     '11111111-1111-1111-1111-111111111111'::uuid,
     2026, 2, 'estadual',
     DATE '2026-10-25', DATE '2026-08-15', now())
ON CONFLICT (org_id, year, round, sphere) DO NOTHING;

COMMIT;

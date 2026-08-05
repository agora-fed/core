-- Migration 0505 — seed of the 4 structural elections for 2026.
--
-- Not a schema migration (no DDL), it is an idempotent seed. Creates the 4
-- rows in `election` the `/eleicoes/2026` comparator needs in order to
-- list the races:
--
--   1. Federal — 1st round (2026-10-04): President + Senator +
--      Federal Deputy
--   2. Federal — 2nd round (2026-10-25): President (if any)
--   3. State — 1st round (2026-10-04): Governor + State Deputy
--   4. State — 2nd round (2026-10-25): Governor (if any)
--
-- Dates follow the official TSE calendar. registration_deadline = 2026-08-15
-- (candidacy registration cut-off, TSE Resolution 23.735/2024).
--
-- Idempotent via UNIQUE(org_id, year, round, sphere).

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
    -- Federal — 1st round
    ('a0000001-0000-4000-8000-000020261001'::uuid,
     '11111111-1111-1111-1111-111111111111'::uuid,
     2026, 1, 'federal',
     DATE '2026-10-04', DATE '2026-08-15', now()),
    -- Federal — 2nd round
    ('a0000002-0000-4000-8000-000020261002'::uuid,
     '11111111-1111-1111-1111-111111111111'::uuid,
     2026, 2, 'federal',
     DATE '2026-10-25', DATE '2026-08-15', now()),
    -- State — 1st round
    ('a0000003-0000-4000-8000-000020261003'::uuid,
     '11111111-1111-1111-1111-111111111111'::uuid,
     2026, 1, 'estadual',
     DATE '2026-10-04', DATE '2026-08-15', now()),
    -- State — 2nd round
    ('a0000004-0000-4000-8000-000020261004'::uuid,
     '11111111-1111-1111-1111-111111111111'::uuid,
     2026, 2, 'estadual',
     DATE '2026-10-25', DATE '2026-08-15', now())
ON CONFLICT (org_id, year, round, sphere) DO NOTHING;

COMMIT;

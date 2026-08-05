-- 0201_mandate_real_parliament — extend `mandate` so it can carry REAL parliamentarian data
-- (party, UF, source api id, official photo). ADR-0010 leans into the consequence loop being
-- testable against the actual Brazilian Congress — without these columns, the directory would
-- show "Mandato sem partido" cards that read as fake.
--
-- Range 0200-0209 = mandates per migrations/REGISTRY.md. Migration 0200 created the table;
-- this is the first additive extension.
--
-- All columns are NULLABLE so:
--   1. The 5 fictional seed rows from earlier sessions keep working.
--   2. A future hand-created "candidacy" without source-api-id is still valid.
--   3. The seed script can rerun and only fills what it knows.

ALTER TABLE mandate
    -- Party abbreviation as it appears in the official base (PT, PCdoB, PSOL, PV, REDE…). Case
    -- preserved so the UI can display "PCdoB" with the lowercase d.
    ADD COLUMN party text,
    -- State abbreviation (BA, SP, RJ…). Two chars but stored as text for stability.
    ADD COLUMN uf text,
    -- Casa do Congresso: 'camara' | 'senado'. Drives the UI filter and the cargo string.
    ADD COLUMN house text CHECK (house IS NULL OR house IN ('camara', 'senado')),
    -- Opaque object key in MinIO under bucket `dsoc-media`, e.g.
    -- `mandates/<external_id>/<random>.jpg`. The federation surface renders the URL as
    -- `<MEDIA_BASE_URL>/<key>`. NULL = no official photo yet → UI falls back to initials.
    ADD COLUMN avatar_object_key text,
    -- ID stable in the SOURCE api (Câmara: dadosabertos.camara.leg.br, Senado:
    -- legis.senado.leg.br). Lets the seed script re-run idempotently: ON CONFLICT on
    -- (source, source_external_id) is the natural key.
    ADD COLUMN source text CHECK (source IS NULL OR source IN ('camara', 'senado', 'manual')),
    ADD COLUMN source_external_id text;

-- Unique on the natural key when present. NULLs are allowed (the fictional seed rows have
-- neither, so they don't collide).
CREATE UNIQUE INDEX mandate_source_external_unique
    ON mandate (source, source_external_id)
 WHERE source IS NOT NULL AND source_external_id IS NOT NULL;

-- Faster filter by party for the front-end directory ("show me all PT deputies").
CREATE INDEX mandate_party_idx ON mandate (org_id, party) WHERE party IS NOT NULL;

COMMENT ON COLUMN mandate.party IS
    'Sigla do partido (preservando case: PT, PCdoB, PSOL, PV, REDE...).';
COMMENT ON COLUMN mandate.house IS
    'camara | senado | NULL (mandatos fictícios / candidaturas independentes).';
COMMENT ON COLUMN mandate.source IS
    'Origem dos dados (camara, senado, manual). Combinado com source_external_id é a chave natural pro seed re-rodar.';

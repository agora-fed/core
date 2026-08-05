-- 0652_citizen_residencia.sql — the citizen's declared domicile (state + IBGE municipality).
--
-- Collected MANDATORILY at signup (front and backend both require it). It is the project's
-- territorial axis — the scope for sortition and for municipal/state federation —
-- independent of the voter registration card (optional and sparse). Nullable in the schema
-- (old rows have none; backfilled through the profile nudge); the requirement is
-- enforced at the signup boundary, not in the database.
--
-- `uf` is denormalised from `municipio_ibge` for fast filtering by state.
-- Idempotent: rerun-safe.

BEGIN;

ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS uf text
        CHECK (uf IS NULL OR uf ~ '^[A-Z]{2}$'),
    ADD COLUMN IF NOT EXISTS municipio_ibge integer
        REFERENCES municipio_ibge (codigo_ibge);

COMMENT ON COLUMN citizen.uf IS
    '0652: UF de domicílio (obrigatória na borda do cadastro; denormalizada de municipio_ibge).';
COMMENT ON COLUMN citizen.municipio_ibge IS
    '0652: município de domicílio (FK municipio_ibge.codigo_ibge), declarado no cadastro.';

COMMIT;

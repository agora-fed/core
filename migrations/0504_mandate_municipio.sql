-- Migration 0504 — municipio column on mandate + composite index for the
-- filtered browser (0.23.0-municipais).
--
-- With ~68k vereadores + ~11k prefeitos/vices coming in the next seed, the
-- browser MUST filter server-side (loading every row is 700+ round-trips).
-- The filtered query is (org_id, sphere, uf, municipio) — this migration adds
-- both the column and the covering index.
--
-- Also loosens `mandate.house` CHECK once more to accept 'camara_municipal'
-- (vereadores) and 'executivo_municipal' (prefeitos/vice-prefeitos), matching
-- the shape we already use for the other spheres.

BEGIN;

ALTER TABLE mandate
    ADD COLUMN IF NOT EXISTS municipio text;

-- Composite index for the browser's filtered query.
CREATE INDEX IF NOT EXISTS mandate_sphere_uf_municipio_idx
    ON mandate (org_id, sphere, uf, municipio, display_name);

-- Extra CHECK values for house — vereadores + executivo municipal.
ALTER TABLE mandate
    DROP CONSTRAINT IF EXISTS mandate_house_check;
ALTER TABLE mandate
    ADD CONSTRAINT mandate_house_check
    CHECK (house IS NULL OR house IN (
        'camara', 'senado', 'assembleia',
        'camara_municipal', 'executivo_municipal'
    ));

COMMENT ON COLUMN mandate.municipio IS
    '0.23.0-municipais: município do mandato quando sphere = municipal. NULL para federais/estaduais.';

COMMIT;

-- Migration 0503 — allow state legislatures in the mandate registry.
--
-- Until this migration, `mandate.house` was CHECK'd to ('camara', 'senado')
-- and `mandate.source` to ('camara', 'senado', 'manual'). Adding state
-- legislators (deputados estaduais / distritais) needs a new `house` value
-- 'assembleia' + new `source` values 'assembleia' (from an Assembleia's own
-- open data feed) and 'tse' (from the unified TSE elected-candidates CSV).
--
-- We drop the old CHECK constraints and recreate with the extended value
-- sets. Existing rows are unaffected because their values remain in the new
-- set.

BEGIN;

ALTER TABLE mandate
    DROP CONSTRAINT IF EXISTS mandate_house_check;
ALTER TABLE mandate
    ADD CONSTRAINT mandate_house_check
    CHECK (house IS NULL OR house IN ('camara', 'senado', 'assembleia'));

ALTER TABLE mandate
    DROP CONSTRAINT IF EXISTS mandate_source_check;
ALTER TABLE mandate
    ADD CONSTRAINT mandate_source_check
    CHECK (source IS NULL OR source IN ('camara', 'senado', 'assembleia', 'tse', 'manual'));

COMMENT ON COLUMN mandate.house IS
    '0.22.0: chamber the mandate belongs to. camara=Câmara Federal, senado=Senado, assembleia=Assembleia Estadual/Distrital.';
COMMENT ON COLUMN mandate.source IS
    '0.22.0: origin of the row. assembleia=state open-data feed; tse=unified TSE elected-candidates CSV.';

COMMIT;

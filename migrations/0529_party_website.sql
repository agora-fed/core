-- Migration 0529 — site oficial do partido.
--
-- A `party` já tem `logo_url` (vazio até agora); faltava o site oficial pra
-- exibir na página do partido. O super-admin (SOCRATES) edita ambos.

BEGIN;

ALTER TABLE party ADD COLUMN website text;

COMMENT ON COLUMN party.website IS
    '0.41.0: URL do site oficial do partido (exibido na página do partido).';

COMMIT;

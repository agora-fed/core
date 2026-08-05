-- 0651_municipio_ibge.sql — reference table of Brazilian municipalities (IBGE).
--
-- Source: servicodados.ibge.gov.br/api/v1/localidades/municipios (5,571 municipalities,
-- 27 states). Data populated by `scripts/seed-municipios-ibge.sql` (idempotent).
-- Serves as the FK for the citizen's declared domicile (0652) and feeds the
-- state→municipality selector at signup via `GET /api/v1/municipios?uf=XX`.
--
-- Idempotent: rerun-safe.

BEGIN;

CREATE TABLE IF NOT EXISTS municipio_ibge (
    codigo_ibge integer PRIMARY KEY,        -- 7-digit IBGE code (e.g. 3550308)
    nome        text NOT NULL,              -- official name (e.g. 'São Paulo')
    uf          text NOT NULL               -- state abbreviation (e.g. 'SP')
        CHECK (uf ~ '^[A-Z]{2}$')
);

-- The signup selector filters by state and sorts by name.
CREATE INDEX IF NOT EXISTS municipio_ibge_uf_nome_idx
    ON municipio_ibge (uf, nome);

COMMENT ON TABLE municipio_ibge IS
    '0651: referência IBGE de municípios (código+nome+UF). Povoada por scripts/seed-municipios-ibge.sql.';

COMMIT;

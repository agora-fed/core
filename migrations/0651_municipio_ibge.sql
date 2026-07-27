-- 0651_municipio_ibge.sql — tabela de referência dos municípios brasileiros (IBGE).
--
-- Fonte: servicodados.ibge.gov.br/api/v1/localidades/municipios (5.571 municípios,
-- 27 UFs). Dados povoados por `scripts/seed-municipios-ibge.sql` (idempotente).
-- Serve de FK pro domicílio declarado do cidadão (0652) e alimenta o selector
-- UF→município do cadastro via `GET /api/v1/municipios?uf=XX`.
--
-- Idempotente: rerun-safe.

BEGIN;

CREATE TABLE IF NOT EXISTS municipio_ibge (
    codigo_ibge integer PRIMARY KEY,        -- código IBGE de 7 dígitos (ex. 3550308)
    nome        text NOT NULL,              -- nome oficial (ex. 'São Paulo')
    uf          text NOT NULL               -- sigla da UF (ex. 'SP')
        CHECK (uf ~ '^[A-Z]{2}$')
);

-- O selector do cadastro filtra por UF e ordena por nome.
CREATE INDEX IF NOT EXISTS municipio_ibge_uf_nome_idx
    ON municipio_ibge (uf, nome);

COMMENT ON TABLE municipio_ibge IS
    '0651: referência IBGE de municípios (código+nome+UF). Povoada por scripts/seed-municipios-ibge.sql.';

COMMIT;

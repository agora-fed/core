-- 0522_electorate.sql — eleitorado oficial TSE por território (item 4 do
-- plano: threshold dinâmico). O gatilho de consequência deixa de ser um
-- número escolhido pelo autor e passa a ser fração do eleitorado do
-- território do mandato, com piso/teto — legitimidade estatística.
-- Seed: scripts/seed-eleitorado-tse.py (perfil_eleitorado do TSE).
-- Linhas: (uf, municipio) por município; (uf, NULL) total da UF;
-- ('BR', NULL) total nacional.

CREATE TABLE electorate (
    id          uuid PRIMARY KEY,
    uf          text NOT NULL,
    municipio   text,
    voters      bigint NOT NULL CHECK (voters >= 0),
    updated_at  timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX electorate_territory_uidx
    ON electorate (uf, COALESCE(municipio, ''));

COMMENT ON TABLE electorate IS
    '0.30.1: eleitorado oficial TSE por território — base do threshold dinâmico.';

ALTER TABLE electorate OWNER TO dsoc;

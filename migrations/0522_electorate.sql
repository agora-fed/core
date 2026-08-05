-- 0522_electorate.sql — official TSE electorate per territory (item 4 of the
-- plan: dynamic threshold). The consequence trigger stops being a number
-- chosen by the author and becomes a fraction of the electorate of the
-- mandate's territory, with a floor/ceiling — statistical legitimacy.
-- Seed: scripts/seed-eleitorado-tse.py (TSE perfil_eleitorado).
-- Rows: (uf, municipio) per municipality; (uf, NULL) state total;
-- ('BR', NULL) national total.

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

-- Migration 0502 — elections + candidacies + gender (Roadmap Fase 3/4 — 2026 elections).
--
-- Adds the base tables the /eleicoes/2026 comparador (Fase 4 of the plan) will
-- draw on:
--
--   * `citizen.gender` and `mandate.gender` — enum {mulher, homem, nao-binarie,
--     prefiro-nao-dizer}. Nullable because we only fill it when the TSE-derived
--     dataset or the citizen themselves provides it.
--   * `election` — one row per federal/estadual/municipal round.
--   * `candidacy` — junction between a mandate and an election (a mandate can
--     have several candidacies over its life; each candidacy carries the
--     election-time party affiliation, ballot number and geographic sphere).
--
-- Data source (later): tse.jus.br public CSV (Divulgação de Candidaturas).
-- Backfill runs as a separate script; this migration only shapes the schema.

BEGIN;

-- ─────────────────────────────────────────────────────────────
-- Gender (nullable)
-- ─────────────────────────────────────────────────────────────
-- Add as text with CHECK rather than a real enum so future values (e.g. a
-- refined non-binary spectrum) don't force a schema-migration for every
-- variant. NULL = citizen hasn't stated / TSE record didn't specify.
ALTER TABLE citizen
    ADD COLUMN gender text
    CHECK (gender IS NULL OR gender IN ('mulher','homem','nao-binarie','prefiro-nao-dizer'));

ALTER TABLE mandate
    ADD COLUMN gender text
    CHECK (gender IS NULL OR gender IN ('mulher','homem','nao-binarie','prefiro-nao-dizer'));

COMMENT ON COLUMN citizen.gender IS
    '0.21.0-alpha: self-declared gender; NULL = not stated. See mandate.gender for TSE-derived.';
COMMENT ON COLUMN mandate.gender IS
    '0.21.0-alpha: TSE-derived gender from candidate registration. NULL = unknown.';

-- ─────────────────────────────────────────────────────────────
-- election
-- ─────────────────────────────────────────────────────────────
CREATE TABLE election (
    id            uuid PRIMARY KEY,
    org_id        uuid NOT NULL REFERENCES org(id),
    -- e.g. 2026, 2028.
    year          integer NOT NULL CHECK (year BETWEEN 2000 AND 3000),
    -- 1 or 2 (majoritary second round).
    round         integer NOT NULL DEFAULT 1 CHECK (round IN (1,2)),
    -- 'federal' | 'estadual' | 'municipal'. Same axis as mandates.sphere.
    sphere        text NOT NULL CHECK (sphere IN ('federal','estadual','municipal')),
    -- Registration deadline / election day (registration → election_day).
    election_day  date NOT NULL,
    -- Registration deadline for candidacies (TSE calendar).
    registration_deadline date,
    created_at    timestamptz NOT NULL DEFAULT now(),
    UNIQUE (org_id, year, round, sphere)
);

CREATE INDEX election_year_sphere_idx
    ON election (year, sphere);

COMMENT ON TABLE election IS
    '0.21.0-alpha: an election round (year × round × sphere).';

-- ─────────────────────────────────────────────────────────────
-- candidacy
-- ─────────────────────────────────────────────────────────────
CREATE TABLE candidacy (
    id            uuid PRIMARY KEY,
    election_id   uuid NOT NULL REFERENCES election(id) ON DELETE CASCADE,
    -- The mandate whose incarnation is running. Nullable while we bulk-import
    -- before every candidate has a matched mandate row — a batch job later
    -- back-fills the FK once the mandate seed catches up.
    mandate_id    uuid REFERENCES mandate(id),
    -- Snapshot at candidacy filing (a mandate can switch parties mid-life).
    party_sigla   text NOT NULL,
    -- Party org id at filing (soft ref — party rows are keyed by (org_id, sigla)).
    party_org_id  uuid,
    -- Office sought: 'presidente' | 'governador' | 'senador' | 'deputado_federal' |
    -- 'deputado_estadual' | 'prefeito' | 'vice_prefeito' | 'vereador'.
    office        text NOT NULL,
    -- Ballot number.
    number        text NOT NULL,
    -- Geographic sphere at time of candidacy.
    sphere_uf     text CHECK (sphere_uf IS NULL OR length(sphere_uf) = 2),
    sphere_municipio text,
    -- Denormalized fields to keep the comparador query fast without joins.
    candidate_name text NOT NULL,
    candidate_gender text
        CHECK (candidate_gender IS NULL OR
               candidate_gender IN ('mulher','homem','nao-binarie','prefiro-nao-dizer')),
    -- Ranking within their (election, office, sphere) group. NULL until TSE result publishes.
    result_rank   integer,
    -- Free-form status: 'apta', 'indeferida', 'eleita', etc. Keeps the schema
    -- forward-compatible with whatever the TSE feed labels.
    status        text,
    created_at    timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX candidacy_election_office_idx
    ON candidacy (election_id, office, party_sigla);
CREATE INDEX candidacy_mandate_idx
    ON candidacy (mandate_id);
CREATE INDEX candidacy_sphere_uf_idx
    ON candidacy (sphere_uf, sphere_municipio);

COMMENT ON TABLE candidacy IS
    '0.21.0-alpha: candidate registration snapshot for one (mandate, election).';

COMMIT;

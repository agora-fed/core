-- 0671_socrates_sweep — SOCRATES v2: AUTOMATIC sweep of Legislative Ideas.
--
-- The MVP (0670) was admin-curated: someone pasted the idea's URL into the panel. v2
-- discovers the TRENDING e-Cidadania ideas on its own from two of
-- the Senate's public sources (the `restcolecaomaisideia` JSON API and the
-- `principalideia` page), mirrors the new ones and RE-SYNCS the support counter of
-- those already mirrored — the support count is the only dynamic datum that matters
-- (20,000 supporters = the idea becomes a formal legislative suggestion).
--
-- So `socrates_mirror` gains:
--   * `apoiamentos`        — the counter AS THE SENATE FORMATS IT ("20.771"): storing
--                            text avoids inventing thousands-separator parsing and keeps the
--                            topic body faithful to the source;
--   * `porcentagem_favor`  — the favourability index the collection returns;
--   * `apoios_updated_at`  — when the two above were last read
--                            (NULL = never synced, the case of the 0670 mirrors);
--   * `origin`             — 'manual' (an admin pasted it) × 'sweep' (discovered by the
--                            worker). The 'manual' default preserves the 0670 history.
--
-- `socrates_sweep_run` is the log of each round: how many ideas the round SAW
-- (`found`), how many became new topics (`mirrored`), how many were skipped for already
-- existing/exceeding the cap (`skipped`) and the consolidated error, when there was one.
-- Without that log there is no way to tell "the Senate published nothing new" from "the
-- sweep has been broken for three days" — the two look alike in the forum.
--
-- OWNER: ALTER TABLE socrates_mirror OWNER TO dsoc
-- OWNER: ALTER TABLE socrates_sweep_run OWNER TO dsoc
--
-- Idempotente: rerun-safe (IF NOT EXISTS / DO block no CHECK).

BEGIN;

ALTER TABLE socrates_mirror ADD COLUMN IF NOT EXISTS apoiamentos       text;
ALTER TABLE socrates_mirror ADD COLUMN IF NOT EXISTS porcentagem_favor int;
ALTER TABLE socrates_mirror ADD COLUMN IF NOT EXISTS apoios_updated_at timestamptz;
ALTER TABLE socrates_mirror ADD COLUMN IF NOT EXISTS origin            text NOT NULL DEFAULT 'manual';

-- `ADD CONSTRAINT` does not accept IF NOT EXISTS; the DO block keeps it rerun-safe.
DO $$
BEGIN
    ALTER TABLE socrates_mirror
        ADD CONSTRAINT socrates_mirror_origin_chk CHECK (origin IN ('manual', 'sweep'));
EXCEPTION
    WHEN duplicate_object THEN NULL;
END
$$;

CREATE TABLE IF NOT EXISTS socrates_sweep_run (
    id          uuid PRIMARY KEY,
    -- Written at the start of the round: a round in progress already shows in the panel.
    started_at  timestamptz NOT NULL DEFAULT now(),
    -- NULL while the round has not closed (or if the process died midway).
    finished_at timestamptz,
    found       int NOT NULL DEFAULT 0,
    mirrored    int NOT NULL DEFAULT 0,
    skipped     int NOT NULL DEFAULT 0,
    -- Consolidated errors of the round (fetch/parse/mirror); NULL = a clean round.
    error       text
);

-- The panel always reads "the latest rounds".
CREATE INDEX IF NOT EXISTS socrates_sweep_run_started_idx
    ON socrates_sweep_run (started_at DESC);

ALTER TABLE socrates_mirror    OWNER TO dsoc;
ALTER TABLE socrates_sweep_run OWNER TO dsoc;

COMMIT;

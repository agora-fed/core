-- Migration 0105 — the citizen's electoral registry (political verification).
--
-- Creates the `titulo_eleitor` column on `citizen`, attesting the person is a
-- Brazilian citizen eligible to vote (a registry valid at the electoral authority). Together with
-- `titulo_status` — algorithmic (check digits) or verified
-- (a cross-check against a future official source, e.g. TSE open data).
--
-- Rule: only a citizen with `titulo_status = 'validated'` or 'verified' may
-- vote on an urgent agenda item (slice D) — it separates civic participation (any
-- citizen) from binding decision (a verified citizen eligible to vote
-- no Brasil real).
--
-- Idempotente: rerun-safe.

BEGIN;

ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS titulo_eleitor text,
    ADD COLUMN IF NOT EXISTS titulo_status text
        CHECK (titulo_status IS NULL OR titulo_status IN
               ('unverified','validated','verified'));

-- Partial UNIQUE: the same registry cannot appear on two accounts (a basic
-- block against sock puppets). NULLs do not collide with each other (the WHERE clause).
CREATE UNIQUE INDEX IF NOT EXISTS citizen_titulo_eleitor_unique
    ON citizen (titulo_eleitor)
    WHERE titulo_eleitor IS NOT NULL;

COMMENT ON COLUMN citizen.titulo_eleitor IS
    '0.25.0-fediverso: 12 dígitos do título de eleitor TSE (formato SEQ + UF + DVs).';
COMMENT ON COLUMN citizen.titulo_status IS
    '0.25.0-fediverso: unverified | validated (dígitos OK) | verified (cross-check TSE).';

COMMIT;

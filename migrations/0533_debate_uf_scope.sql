-- Migration 0533 — optional territorial scope (state) on debates (0.48.0, Phase 3.1).
--
-- Debates were org-global and flat: the citizen could not find what is RELEVANT to them
-- (the retention risk flagged in the plan — "without 'my state' it does not engage"). An
-- optional state lets a debate be national (NULL) OR belong to state X, and the list filters
-- by "my state". Minimal: state only (no municipality), client-side filter.

BEGIN;

ALTER TABLE debate
    ADD COLUMN uf text
    CONSTRAINT debate_uf_format CHECK (uf IS NULL OR uf ~ '^[A-Z]{2}$');

COMMENT ON COLUMN debate.uf IS
    '0.48.0: UF opcional de escopo territorial do debate (NULL = nacional).';

COMMIT;

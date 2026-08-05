-- Migration 0528 — soft-hide for the super-admin (SOCRATES).
--
-- "Delete" in the admin console HIDES by default (reversible, preserves the record
-- — consistent with "silence is permanent"); a second button performs the cascading
-- hard-delete (irreversible) when the admin really means to destroy.
--
-- Only the soft-hide lives here: `hidden_at NULL` = visible. Public reads now
-- filter `hidden_at IS NULL`. The hard-delete is a cascading DELETE in code
-- (needs no column).

BEGIN;

ALTER TABLE mandate  ADD COLUMN hidden_at timestamptz;
ALTER TABLE proposal ADD COLUMN hidden_at timestamptz;

-- Partial indexes: public reads filter the NON-hidden rows (the common case).
CREATE INDEX mandate_visible_idx  ON mandate  (org_id) WHERE hidden_at IS NULL;
CREATE INDEX proposal_visible_idx ON proposal (org_id) WHERE hidden_at IS NULL;

COMMENT ON COLUMN mandate.hidden_at IS
    '0.40.0: quando != NULL, o mandato foi ocultado por um admin (some das leituras públicas).';
COMMENT ON COLUMN proposal.hidden_at IS
    '0.40.0: quando != NULL, a proposta foi ocultada por um admin.';

COMMIT;

-- Migration 0529 — the party's official website.
--
-- `party` already has `logo_url` (empty so far); the official website was missing
-- for display on the party page. The super-admin (SOCRATES) edits both.

BEGIN;

ALTER TABLE party ADD COLUMN website text;

COMMENT ON COLUMN party.website IS
    '0.41.0: URL do site oficial do partido (exibido na página do partido).';

COMMIT;

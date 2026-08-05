-- Migration 0152 — optional party affiliation of an ordinary citizen.
--
-- Context: until now the platform distinguished only:
--   - mandate.party  → the official's party in their mandate
--   - party_administrator → a citizen who administers a party
-- The most common case was missing: an ordinary citizen simply saying "I am PT",
-- without becoming an admin, without holding a mandate. It signals to the rest of the UI ("my
-- party") + serves scorecard/party filters in the admin.
--
-- Nullable — the affiliation is optional. A soft FK (validated only when filled): that way
-- officially adding/removing parties never breaks historical data.

BEGIN;

-- No FK: the party PK is (org_id, sigla), and enforcing that on citizen would require
-- carrying the citizen's org_id into the INSERT/UPDATE (the UI does a join). Practical
-- validation comes from the front-end dropdown (which reads /parties) and the ORG-level
-- consistency the platform already maintains.
ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS party_sigla text;

CREATE INDEX IF NOT EXISTS citizen_party_sigla_idx
    ON citizen (party_sigla)
    WHERE party_sigla IS NOT NULL;

COMMENT ON COLUMN citizen.party_sigla IS
    '0.25.0-fediverso: filiação partidária opcional (só informativo, sem permissões). FK em party.sigla.';

COMMIT;

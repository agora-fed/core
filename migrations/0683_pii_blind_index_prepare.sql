-- 0683_pii_blind_index_prepare.sql — make the cleartext identifiers droppable (AGORA #15).
--
-- The contract half of 0682, split in two because a column cannot be dropped while the
-- running image still writes it. This migration only makes the cleartext OPTIONAL and
-- gives the voter registration the same blind index the CPF already has; 0684 does the
-- dropping, after a deploy that stops writing them.
--
-- Why voter registration needs one too: it carries a UNIQUE index of its own
-- (`citizen_titulo_eleitor_unique`), which 0682 overlooked. Dropping the column without
-- replacing that index would silently allow the same voter registration on two
-- accounts — trading a confidentiality problem for an integrity one.
--
-- The HMAC is computed with pgcrypto's `hmac(...)`, which was verified byte-for-byte
-- against the application's Rust implementation before this migration was written:
-- both produce 05f0f406… for ('39053344705', 'chave-de-teste'). Had they differed, the
-- backfilled rows would have been invisible to the application's own lookups.
--
-- Idempotent: rerun-safe.

BEGIN;

-- ---------------------------------------------------------------------------
-- 1. CPF: the cleartext stops being mandatory, and its UNIQUE gives way to the
--    blind index, which already carries the same guarantee.
-- ---------------------------------------------------------------------------
ALTER TABLE auth_credential ALTER COLUMN cpf DROP NOT NULL;

-- The CONSTRAINT owns the index, so it goes first — dropping the index directly
-- fails with "cannot drop index ... because constraint ... requires it".
ALTER TABLE auth_credential DROP CONSTRAINT IF EXISTS auth_credential_org_id_cpf_key;
DROP INDEX IF EXISTS auth_credential_org_id_cpf_key;

-- ---------------------------------------------------------------------------
-- 1b. The masked CPF the admin list renders.
--
-- The CPF is NOT purely write-only, contrary to what 0682's header assumed:
-- `/admin/users-rich` reads it to build `123.***.***-09`. Deriving that from the
-- ciphertext would mean decrypting every row on every page of an admin list, to
-- produce a string that is already public at the API edge. So it is stored as what
-- it is — a mask — and computed HERE, while the cleartext still exists.
-- ---------------------------------------------------------------------------
ALTER TABLE auth_credential ADD COLUMN IF NOT EXISTS cpf_masked text;

UPDATE auth_credential
   SET cpf_masked = left(cpf, 3) || '.***.***-' || right(cpf, 2)
 WHERE cpf IS NOT NULL AND length(cpf) >= 5 AND cpf_masked IS NULL;

COMMENT ON COLUMN auth_credential.cpf_masked IS
    '0683 (#15): the masked form the admin list shows. Public at the edge; stored so reading it needs no key.';

-- ---------------------------------------------------------------------------
-- 2. Voter registration: its own blind index, mirroring the CPF's.
--    Global (not per-org) because `citizen_titulo_eleitor_unique` was global — one
--    voter registration belongs to one person nationally, not one per tenant.
-- ---------------------------------------------------------------------------
ALTER TABLE citizen ADD COLUMN IF NOT EXISTS titulo_hmac bytea;

CREATE UNIQUE INDEX IF NOT EXISTS citizen_titulo_hmac_uidx
    ON citizen (titulo_hmac)
    WHERE titulo_hmac IS NOT NULL;

COMMENT ON COLUMN citizen.titulo_hmac IS
    '0683 (#15): HMAC-SHA256 of the digits of the voter registration. Carries the uniqueness that citizen_titulo_eleitor_unique held over the cleartext.';

COMMIT;

-- 0684_pii_drop_cleartext.sql — the identifiers stop existing in the clear (AGORA #15).
--
-- The contract step. 0682 added the protected columns, 0683 made the cleartext optional
-- and gave the voter registration its own blind index, and the application stopped
-- writing either in v0.81.0. This drops what is left.
--
-- After this migration the cleartext is GONE, and the only way back to a CPF or a voter
-- registration is `PII_ENCRYPTION_KEY`. Losing that key loses the values — which is
-- the point, and also the risk. It belongs in the same custody plan as the rest of the
-- production secrets (#43).
--
-- THE GUARD BELOW IS NOT CEREMONY. Applying this before the backfill would drop the
-- cleartext of rows whose blind index was never computed: uniqueness silently stops
-- being enforced for them, and the value is unrecoverable. So the migration refuses
-- unless every row that HAS a cleartext value also has its index. On a fresh database
-- there are no rows and it passes trivially.
--
-- Idempotent: rerun-safe.

BEGIN;

DO $$
DECLARE
    cpf_missing    bigint;
    titulo_missing bigint;
BEGIN
    -- Column may already be gone on a re-run; guard the guard.
    IF EXISTS (SELECT 1 FROM information_schema.columns
                WHERE table_name = 'auth_credential' AND column_name = 'cpf') THEN
        EXECUTE 'SELECT count(*) FROM auth_credential WHERE cpf IS NOT NULL AND cpf_hmac IS NULL'
           INTO cpf_missing;
        IF cpf_missing > 0 THEN
            RAISE EXCEPTION
                '0684: % credential(s) still hold a cleartext CPF with no blind index. Run the backfill with PII_ENCRYPTION_KEY first, or these values are lost and their uniqueness with them.',
                cpf_missing;
        END IF;
    END IF;

    IF EXISTS (SELECT 1 FROM information_schema.columns
                WHERE table_name = 'citizen' AND column_name = 'titulo_eleitor') THEN
        EXECUTE 'SELECT count(*) FROM citizen WHERE titulo_eleitor IS NOT NULL AND titulo_hmac IS NULL'
           INTO titulo_missing;
        IF titulo_missing > 0 THEN
            RAISE EXCEPTION
                '0684: % citizen(s) still hold a cleartext voter registration with no blind index. Run the backfill first.',
                titulo_missing;
        END IF;
    END IF;
END $$;

-- The old global UNIQUE goes with the column it indexed; `citizen_titulo_hmac_uidx`
-- (0683) already carries the same guarantee over the blind index.
DROP INDEX IF EXISTS citizen_titulo_eleitor_unique;

ALTER TABLE auth_credential DROP COLUMN IF EXISTS cpf;
ALTER TABLE citizen         DROP COLUMN IF EXISTS titulo_eleitor;

COMMENT ON COLUMN auth_credential.cpf_enc IS
    '0684 (#15): the ONLY remaining form of the CPF. Readable only with PII_ENCRYPTION_KEY.';

COMMIT;

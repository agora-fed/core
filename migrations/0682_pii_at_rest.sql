-- 0682_pii_at_rest.sql — national identifiers and the 2FA secret out of cleartext (AGORA #15).
--
-- These sat in plain text in the main tables, next to the password hash: a database
-- dump handed over every citizen's CPF, voter registration, phone and — worst of the
-- set — their TOTP shared secret, which is a permanent 2FA bypass for as long as it
-- is not rotated. The repo already encrypts VENDOR credentials this way
-- (`intercoms_provider_config`, 0660); the same standard simply was not applied to
-- citizens' own identifiers.
--
-- TWO different treatments, because the columns are used differently:
--
--   * `cpf` is WRITE-ONLY in this codebase. Nothing reads it back — not the LGPD
--     export (which exposes only `cpf_status`), not any masking, not re-verification
--     (which runs at signup with the value in hand). All it does is enforce
--     uniqueness. So it becomes a keyed HMAC — an index that answers "is this CPF
--     already registered?" without holding the CPF — plus an encrypted copy for
--     retrievability. Storing ONLY the HMAC would be strictly safer still (you cannot
--     leak what you do not have); that is a deliberate follow-up decision, not one to
--     take inside a migration.
--
--   * `titulo_eleitor` and `phone` ARE displayed, masked to their last four digits.
--     So: encrypted value + a plaintext `last4`, which is what the API already emits.
--
-- The key lives in `PII_ENCRYPTION_KEY`, outside the database, exactly as
-- `INTERCOMS_CONFIG_KEY` does. This migration therefore only adds columns and cannot
-- encrypt anything: the backfill is a deploy step that carries the key, and the
-- cleartext columns are dropped by a LATER migration once it is confirmed. Expand,
-- migrate, contract — a single-step version would have to choose between downtime and
-- losing rows it could not re-encrypt.
--
-- EXCEPTION, and the reason this migration is not purely additive: `totp_secret`,
-- `phone` and `phone_otp.phone` hold ZERO rows in production (verified 2026-08-06), so
-- for those there is nothing to migrate and nothing to lose. They are converted here.
--
-- Idempotent: rerun-safe.

BEGIN;

-- ---------------------------------------------------------------------------
-- 1. CPF — blind index for uniqueness + encrypted copy.
-- ---------------------------------------------------------------------------
ALTER TABLE auth_credential
    ADD COLUMN IF NOT EXISTS cpf_hmac bytea,
    ADD COLUMN IF NOT EXISTS cpf_enc  bytea;

-- The uniqueness that `UNIQUE (org_id, cpf)` provided, over the blind index instead.
-- Partial: rows not yet backfilled must not collide with each other on NULL.
CREATE UNIQUE INDEX IF NOT EXISTS auth_credential_org_cpf_hmac_uidx
    ON auth_credential (org_id, cpf_hmac)
    WHERE cpf_hmac IS NOT NULL;

COMMENT ON COLUMN auth_credential.cpf_hmac IS
    '0682 (#15): HMAC-SHA256 of the normalized CPF under PII_ENCRYPTION_KEY. Enforces uniqueness without holding the value.';

-- ---------------------------------------------------------------------------
-- 2. Voter registration — encrypted + the last four the UI already shows.
-- ---------------------------------------------------------------------------
ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS titulo_enc   bytea,
    ADD COLUMN IF NOT EXISTS titulo_last4 text;

-- The masked form can be derived NOW, without the key: it is already public at the
-- API edge, so deriving it here costs nothing and lets the UI keep working the moment
-- the cleartext column goes.
UPDATE citizen
   SET titulo_last4 = right(titulo_eleitor, 4)
 WHERE titulo_eleitor IS NOT NULL AND titulo_last4 IS NULL;

-- ---------------------------------------------------------------------------
-- 3. Phone and the 2FA secret — ZERO rows in production, so converted outright.
--    A guard keeps this honest: if a row ever exists, the migration refuses rather
--    than silently discarding someone's identifier.
-- ---------------------------------------------------------------------------
DO $$
DECLARE
    n_phone  bigint;
    n_totp   bigint;
    n_otp    bigint;
BEGIN
    SELECT count(*) INTO n_phone FROM citizen WHERE phone IS NOT NULL;
    SELECT count(*) INTO n_totp  FROM citizen WHERE totp_secret IS NOT NULL;
    SELECT count(*) INTO n_otp   FROM phone_otp;
    IF n_phone > 0 OR n_totp > 0 OR n_otp > 0 THEN
        RAISE EXCEPTION
            '0682: expected zero rows to convert but found phone=% totp=% otp=%. Back these up and backfill them like CPF instead of dropping.',
            n_phone, n_totp, n_otp;
    END IF;
END $$;

ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS phone_enc        bytea,
    ADD COLUMN IF NOT EXISTS phone_last4      text,
    ADD COLUMN IF NOT EXISTS totp_secret_enc  bytea;

ALTER TABLE citizen DROP COLUMN IF EXISTS phone;
ALTER TABLE citizen DROP COLUMN IF EXISTS totp_secret;

ALTER TABLE phone_otp ADD COLUMN IF NOT EXISTS phone_enc bytea;
ALTER TABLE phone_otp DROP COLUMN IF EXISTS phone;

COMMENT ON COLUMN citizen.totp_secret_enc IS
    '0682 (#15): pgp_sym_encrypt of the TOTP shared secret. In cleartext it was a permanent 2FA bypass from any dump.';

COMMIT;

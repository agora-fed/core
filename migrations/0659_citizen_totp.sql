-- 0659_citizen_totp.sql — citizen TOTP 2FA (RFC 6238) (AGORA F6, #63).
--
-- TOTP is the **recommended** second factor (authenticator app), opt-in. The base32 secret lives
-- in `citizen.totp_secret`; `totp_enabled_at` marks when it was confirmed. Recovery codes
-- (SHA-256 hashes only) restore access if the citizen loses the app. F6.1 = enrollment/management;
-- enforcing it at login is the next slice (F6.2).
--
-- SECURITY NOTE: `totp_secret` is in the clear in this slice (encryption at rest = follow-up,
-- together with intercoms_provider_config #69). Low risk while TOTP is not required at login.
--
-- Idempotent: rerun-safe.

BEGIN;

ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS totp_secret text,        -- base32 secret (RFC 4648)
    ADD COLUMN IF NOT EXISTS totp_enabled_at timestamptz;

CREATE TABLE IF NOT EXISTS totp_recovery_code (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    citizen_id  uuid NOT NULL REFERENCES citizen(id),
    code_hash   bytea NOT NULL,                       -- SHA-256 of the recovery code
    used_at     timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS totp_recovery_code_citizen_idx
    ON totp_recovery_code (citizen_id)
    WHERE used_at IS NULL;

COMMENT ON TABLE totp_recovery_code IS
    '0659 (F6/#63): códigos de recuperação do TOTP (só hash). Consumidos 1x.';

COMMIT;

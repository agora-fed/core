-- 0658_citizen_phone.sql — citizen phone number + SMS OTP verification (AGORA F5, #62).
--
-- The phone is **opt-in** and verified by an OTP code sent over SMS (via INTERCOMS/SmsGateway,
-- ADR-0016). It enables SMS 2FA (not recommended, an alternative if e-mail is lost) and SMS
-- reach in small municipalities. `phone_otp` stores only the SHA-256 of the code (plaintext is
-- never persisted, same pattern as signup-verify/password-reset).
--
-- Idempotent: rerun-safe.

BEGIN;

ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS phone text,            -- E.164 (e.g. +5511987654321)
    ADD COLUMN IF NOT EXISTS phone_verified_at timestamptz;

CREATE TABLE IF NOT EXISTS phone_otp (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    citizen_id  uuid NOT NULL REFERENCES citizen(id),
    phone       text NOT NULL,
    code_hash   bytea NOT NULL,                      -- SHA-256 of the 6-digit code
    expires_at  timestamptz NOT NULL,               -- short TTL (10 min)
    used_at     timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now()
);

-- "my most recent OTP" (verify) + basis of the per-citizen rate limit.
CREATE INDEX IF NOT EXISTS phone_otp_citizen_idx
    ON phone_otp (citizen_id, created_at DESC);

COMMENT ON TABLE phone_otp IS
    '0658 (F5/#62): OTP de verificação de telefone (só hash; TTL curto). Opt-in.';

COMMIT;

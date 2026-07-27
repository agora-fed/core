-- 0659_citizen_totp.sql — 2FA por TOTP (RFC 6238) do cidadão (ÁGORA F6, #63).
--
-- TOTP é o 2FA **recomendado** (app autenticador), opt-in. O segredo (base32) fica em
-- `citizen.totp_secret`; `totp_enabled_at` marca quando foi confirmado. Códigos de recuperação
-- (só hash SHA-256) para acesso se o cidadão perder o app. F6.1 = enrollment/gestão; forçar no
-- login é a fatia seguinte (F6.2).
--
-- NOTA de segurança: `totp_secret` está em claro nesta fatia (encriptação em repouso = follow-up,
-- junto com intercoms_provider_config #69). Risco baixo enquanto TOTP não é exigido no login.
--
-- Idempotente: rerun-safe.

BEGIN;

ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS totp_secret text,        -- segredo base32 (RFC 4648)
    ADD COLUMN IF NOT EXISTS totp_enabled_at timestamptz;

CREATE TABLE IF NOT EXISTS totp_recovery_code (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    citizen_id  uuid NOT NULL REFERENCES citizen(id),
    code_hash   bytea NOT NULL,                       -- SHA-256 do código de recuperação
    used_at     timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS totp_recovery_code_citizen_idx
    ON totp_recovery_code (citizen_id)
    WHERE used_at IS NULL;

COMMENT ON TABLE totp_recovery_code IS
    '0659 (F6/#63): códigos de recuperação do TOTP (só hash). Consumidos 1x.';

COMMIT;

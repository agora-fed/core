-- 0658_citizen_phone.sql — telefone do cidadão + verificação por OTP SMS (ÁGORA F5, #62).
--
-- Telefone é **opt-in** e verificado por código OTP enviado por SMS (via INTERCOMS/SmsGateway,
-- ADR-0016). Habilita 2FA por SMS (não-recomendada, alternativa em caso de perda de e-mail) e o
-- alcance por SMS em municípios pequenos. `phone_otp` guarda só o SHA-256 do código (plaintext
-- nunca persistido, mesmo padrão do signup-verify/password-reset).
--
-- Idempotente: rerun-safe.

BEGIN;

ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS phone text,            -- E.164 (ex.: +5511987654321)
    ADD COLUMN IF NOT EXISTS phone_verified_at timestamptz;

CREATE TABLE IF NOT EXISTS phone_otp (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    citizen_id  uuid NOT NULL REFERENCES citizen(id),
    phone       text NOT NULL,
    code_hash   bytea NOT NULL,                      -- SHA-256 do código de 6 dígitos
    expires_at  timestamptz NOT NULL,               -- TTL curto (10 min)
    used_at     timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now()
);

-- "meu OTP mais recente" (verify) + base do rate-limit por cidadão.
CREATE INDEX IF NOT EXISTS phone_otp_citizen_idx
    ON phone_otp (citizen_id, created_at DESC);

COMMENT ON TABLE phone_otp IS
    '0658 (F5/#62): OTP de verificação de telefone (só hash; TTL curto). Opt-in.';

COMMIT;

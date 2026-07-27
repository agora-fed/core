-- 0660_intercoms_provider_config.sql — config de provider de comunicação por escopo (INTERCOMS #69).
--
-- Cada diretório cadastra seu PRÓPRIO SMSGateway (host/token) — ADR-0016. As credenciais ficam
-- **cifradas em repouso** com pgcrypto (`pgp_sym_encrypt`, chave em `INTERCOMS_CONFIG_KEY` no
-- Secret; nunca no banco). `directory_id NULL` = escopo plataforma (futuro). Uma config por
-- (diretório, canal). O envio que usa esta config é a fatia seguinte (#69b: broadcast SMS).
--
-- Idempotente: rerun-safe.

BEGIN;

CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE TABLE IF NOT EXISTS intercoms_provider_config (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id       uuid NOT NULL REFERENCES org(id),
    directory_id uuid REFERENCES party_directory(id),
    channel      text NOT NULL CHECK (channel IN ('sms', 'email')),
    provider     text NOT NULL,                 -- ex.: 'smsgateway'
    config       bytea NOT NULL,                -- pgp_sym_encrypt(json, key) — {url,user,pass}
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now()
);

-- Uma config por (diretório, canal). `directory_id` NULL (plataforma) é seu próprio bucket.
CREATE UNIQUE INDEX IF NOT EXISTS intercoms_provider_config_scope_uq
    ON intercoms_provider_config (COALESCE(directory_id, '00000000-0000-0000-0000-000000000000'), channel);

COMMENT ON TABLE intercoms_provider_config IS
    '0660 (#69): config de provider (ex. SMSGateway) por diretório; credenciais cifradas (pgcrypto).';

COMMIT;

-- 0660_intercoms_provider_config.sql — per-scope communication provider config (INTERCOMS #69).
--
-- Each chapter registers its OWN SMSGateway (host/token) — ADR-0016. Credentials are stored
-- **encrypted at rest** with pgcrypto (`pgp_sym_encrypt`, key in `INTERCOMS_CONFIG_KEY` in the
-- Secret; never in the database). `directory_id NULL` = platform scope (future). One config per
-- (chapter, channel). The sending that uses this config is the next slice (#69b: SMS broadcast).
--
-- Idempotent: rerun-safe.

BEGIN;

CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE TABLE IF NOT EXISTS intercoms_provider_config (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id       uuid NOT NULL REFERENCES org(id),
    directory_id uuid REFERENCES party_directory(id),
    channel      text NOT NULL CHECK (channel IN ('sms', 'email')),
    provider     text NOT NULL,                 -- e.g. 'smsgateway'
    config       bytea NOT NULL,                -- pgp_sym_encrypt(json, key) — {url,user,pass}
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now()
);

-- One config per (chapter, channel). `directory_id` NULL (platform) is its own bucket.
CREATE UNIQUE INDEX IF NOT EXISTS intercoms_provider_config_scope_uq
    ON intercoms_provider_config (COALESCE(directory_id, '00000000-0000-0000-0000-000000000000'), channel);

COMMENT ON TABLE intercoms_provider_config IS
    '0660 (#69): config de provider (ex. SMSGateway) por diretório; credenciais cifradas (pgcrypto).';

COMMIT;

-- 0655_campaign_broadcast.sql — history of consented campaign broadcasts (F3, #60).
--
-- A MUNICIPAL chapter sends a message to its consented base (crossing consent
-- 0654 with the citizen's domicile 0652). This table records every send: audit trail + basis for
-- the rate limit (per-chapter cooldown) + recipient count. The raw list is NEVER
-- exported — sending is mediated by the platform (INTERCOMS/SmtpProvider).
--
-- Idempotent: rerun-safe.

BEGIN;

CREATE TABLE IF NOT EXISTS campaign_broadcast (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id       uuid NOT NULL REFERENCES org(id),
    party_sigla  text NOT NULL,
    directory_id uuid NOT NULL REFERENCES party_directory(id),
    sent_by      uuid NOT NULL REFERENCES citizen(id),
    channel      text NOT NULL DEFAULT 'email' CHECK (channel IN ('email', 'sms')),
    subject      text NOT NULL,
    body         text NOT NULL,
    recipients   integer NOT NULL DEFAULT 0,   -- how many consented recipients were reached
    created_at   timestamptz NOT NULL DEFAULT now()
);

-- Per-chapter cooldown (anti-spam): "was there a broadcast from this chapter in the last 24h?"
CREATE INDEX IF NOT EXISTS campaign_broadcast_directory_recent_idx
    ON campaign_broadcast (directory_id, created_at DESC);

COMMENT ON TABLE campaign_broadcast IS
    '0655 (F3/#60): histórico de broadcast consentido por diretório municipal (auditoria + rate-limit).';

COMMIT;

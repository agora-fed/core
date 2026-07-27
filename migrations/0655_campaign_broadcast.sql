-- 0655_campaign_broadcast.sql — histórico de broadcast consentido de campanha (F3, #60).
--
-- Um diretório MUNICIPAL envia uma mensagem à sua base consentida (cruzando o consentimento
-- 0654 com o domicílio do cidadão 0652). Esta tabela registra cada envio: auditoria + base do
-- rate-limit (cooldown por diretório) + contagem de destinatários. A lista crua NUNCA é
-- exportada — o envio é mediado pela plataforma (INTERCOMS/SmtpProvider).
--
-- Idempotente: rerun-safe.

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
    recipients   integer NOT NULL DEFAULT 0,   -- quantos destinatários consentidos foram alcançados
    created_at   timestamptz NOT NULL DEFAULT now()
);

-- Cooldown por diretório (anti-spam): "houve broadcast deste diretório nas últimas 24h?"
CREATE INDEX IF NOT EXISTS campaign_broadcast_directory_recent_idx
    ON campaign_broadcast (directory_id, created_at DESC);

COMMENT ON TABLE campaign_broadcast IS
    '0655 (F3/#60): histórico de broadcast consentido por diretório municipal (auditoria + rate-limit).';

COMMIT;

-- 0521_notification_receipt.sql — o "AR digital do silêncio" (item 2 do
-- plano estratégico, fatia 1). Cada e-mail enviado ao gabinete vira um
-- RECIBO persistido e encadeado por hash: adulterar qualquer recibo quebra
-- a cadeia dali em diante. O silêncio deixa de ser acusação e vira fato
-- auditável: "avisamos em D0, reavisamos em D+1 e D+2, com estes hashes".
--
-- Cadeia POR PROPOSTA (attempt 1→2→3): sequencial por construção, sem
-- corrida de concorrência; genesis = sha256("genesis:<proposal_id>").
-- `proposal_id` é soft-ref (tabela de outro crate — regra do REGISTRY);
-- `recipient` é o public_email oficial do mandato (dado público).

CREATE TABLE notification_receipt (
    id           uuid PRIMARY KEY,
    proposal_id  uuid NOT NULL,
    mandate_id   uuid REFERENCES mandate(id),
    recipient    text NOT NULL,
    -- 1 = D0 (criação/threshold), 2 = D+1, 3 = D+2.
    attempt      integer NOT NULL CHECK (attempt BETWEEN 1 AND 3),
    subject      text NOT NULL,
    -- 'accepted' | 'failed: …' | 'dev-logged' (SMTP não configurado).
    outcome      text NOT NULL,
    sent_at      timestamptz NOT NULL,
    prev_hash    text NOT NULL,
    hash         text NOT NULL,
    UNIQUE (proposal_id, attempt)
);

CREATE INDEX notification_receipt_proposal_idx
    ON notification_receipt (proposal_id, attempt);

COMMENT ON TABLE notification_receipt IS
    '0.29: recibos hash-encadeados dos avisos ao gabinete — prova pública de notificação (AR digital).';

ALTER TABLE notification_receipt OWNER TO dsoc;

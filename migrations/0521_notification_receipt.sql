-- 0521_notification_receipt.sql — the "digital return receipt of silence" (item 2 of the
-- strategic plan, slice 1). Every e-mail sent to a cabinet becomes a persisted
-- RECEIPT chained by hash: tampering with any receipt breaks the chain from
-- there onwards. Silence stops being an accusation and becomes an auditable
-- fact: "we warned on D0, warned again on D+1 and D+2, with these hashes".
--
-- Chain PER PROPOSAL (attempt 1→2→3): sequential by construction, with no
-- concurrency race; genesis = sha256("genesis:<proposal_id>").
-- `proposal_id` is a soft-ref (table owned by another crate — REGISTRY rule);
-- `recipient` is the mandate's official public_email (public data).

CREATE TABLE notification_receipt (
    id           uuid PRIMARY KEY,
    proposal_id  uuid NOT NULL,
    mandate_id   uuid REFERENCES mandate(id),
    recipient    text NOT NULL,
    -- 1 = D0 (creation/threshold), 2 = D+1, 3 = D+2.
    attempt      integer NOT NULL CHECK (attempt BETWEEN 1 AND 3),
    subject      text NOT NULL,
    -- 'accepted' | 'failed: …' | 'dev-logged' (SMTP not configured).
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

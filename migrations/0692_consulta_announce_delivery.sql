BEGIN;

CREATE TABLE IF NOT EXISTS consulta_announce_delivery (
    id                uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    consultation_id   uuid NOT NULL REFERENCES consultations_consultation(id),
    recipient_inbox   text NOT NULL,
    attempts          integer NOT NULL DEFAULT 0,
    sent_at           timestamptz,
    created_at        timestamptz NOT NULL DEFAULT now(),
    UNIQUE (consultation_id, recipient_inbox)
);

CREATE INDEX IF NOT EXISTS consulta_announce_pending_idx
    ON consulta_announce_delivery (sent_at, attempts) WHERE sent_at IS NULL;

ALTER TABLE consulta_announce_delivery OWNER TO dsoc;

COMMIT;

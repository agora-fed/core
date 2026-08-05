-- 0656_broadcast_consultation.sql — links a broadcast to a consultation (F3 slice 2, #60).
--
-- When the chapter includes questions in the message, the broadcast creates a CONSULTATION
-- (reusing `consultations_consultation` + `_question`, ADR-0014) and sends the link to the
-- consented base. Answers/aggregation come for free via the existing /consulta page. This
-- column links the broadcast to the consultation created (so the chapter finds the results).
--
-- Idempotent: rerun-safe.

BEGIN;

ALTER TABLE campaign_broadcast
    ADD COLUMN IF NOT EXISTS consultation_id uuid REFERENCES consultations_consultation(id);

COMMENT ON COLUMN campaign_broadcast.consultation_id IS
    '0656 (F3 fatia 2): consulta criada por este broadcast (micro-consulta), se houve perguntas.';

COMMIT;

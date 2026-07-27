-- 0656_broadcast_consultation.sql — liga um broadcast a uma consulta (F3 fatia 2, #60).
--
-- Quando o diretório inclui perguntas no comunicado, o broadcast cria uma CONSULTA
-- (reusa `consultations_consultation` + `_question`, ADR-0014) e manda o link à base
-- consentida. As respostas/agregação vêm de graça pela página /consulta existente. Esta
-- coluna liga o broadcast à consulta criada (para o diretório achar os resultados).
--
-- Idempotente: rerun-safe.

BEGIN;

ALTER TABLE campaign_broadcast
    ADD COLUMN IF NOT EXISTS consultation_id uuid REFERENCES consultations_consultation(id);

COMMENT ON COLUMN campaign_broadcast.consultation_id IS
    '0656 (F3 fatia 2): consulta criada por este broadcast (micro-consulta), se houve perguntas.';

COMMIT;

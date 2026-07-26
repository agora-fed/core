-- Migration 0650 — moderação de conteúdo de fórum (R3.1 / issue #27, ADR-0011).
--
-- Primeira faixa de MÓDULO (0650+, R0.8): forums. Permite admin/moderador remover
-- tópico e argumento com atribuição + motivo pra audit.
--
-- forum_topic já tem `hidden_at` (some das listagens); só falta a atribuição.
-- forum_topic_comment não tinha flag de ocultação — ganha `hidden_at` + atribuição,
-- e a listagem de comentários passa a filtrar `hidden_at IS NULL`.
--
-- Idempotente: rerun-safe.

BEGIN;

ALTER TABLE forum_topic
    ADD COLUMN IF NOT EXISTS deleted_by      uuid REFERENCES citizen(id),
    ADD COLUMN IF NOT EXISTS deletion_reason text;

ALTER TABLE forum_topic_comment
    ADD COLUMN IF NOT EXISTS hidden_at       timestamptz,
    ADD COLUMN IF NOT EXISTS deleted_by      uuid REFERENCES citizen(id),
    ADD COLUMN IF NOT EXISTS deletion_reason text;

COMMENT ON COLUMN forum_topic.deleted_by IS
    '0650: cidadão (admin/moderador) que removeu o tópico via moderação.';
COMMENT ON COLUMN forum_topic_comment.hidden_at IS
    '0650: quando não-nulo, argumento removido pela moderação — some da listagem.';

COMMIT;

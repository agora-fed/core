-- Migration 0650 — forum content moderation (R3.1 / issue #27, ADR-0011).
--
-- First MODULE band (0650+, R0.8): forums. Lets an admin/moderator remove a
-- topic or an argument, with attribution + reason for the audit trail.
--
-- forum_topic already has `hidden_at` (drops out of listings); only attribution was missing.
-- forum_topic_comment had no hiding flag — it gains `hidden_at` + attribution,
-- and the comment listing starts filtering `hidden_at IS NULL`.
--
-- Idempotent: rerun-safe.

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

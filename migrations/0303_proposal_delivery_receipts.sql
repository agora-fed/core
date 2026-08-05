-- Migration 0303 — proposal delivery receipt (author + mandate).
--
-- A real citizen concern: "I sent it, but did they receive it?". We record
-- timestamps of when the e-mail left the relay — not proof of reading, but
-- the same "delivered" pattern as WhatsApp/corporate e-mail, and far more
-- than we have today (nothing).
--
-- Backfill: old rows stay NULL (we do not send retroactive e-mail).
-- Idempotent on re-run.

BEGIN;

ALTER TABLE proposal
    ADD COLUMN IF NOT EXISTS notified_author_at   timestamptz,
    ADD COLUMN IF NOT EXISTS notified_mandate_at  timestamptz;

COMMENT ON COLUMN proposal.notified_author_at IS
    '0.25.0-fediverso: quando o e-mail de confirmação saiu pro autor.';
COMMENT ON COLUMN proposal.notified_mandate_at IS
    '0.25.0-fediverso: quando o e-mail saiu pro gabinete (mandate.public_email).';

COMMIT;

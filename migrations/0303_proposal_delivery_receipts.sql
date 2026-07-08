-- Migration 0303 — recibo de entrega da proposta (autor + mandato).
--
-- Preocupação real do cidadão: "eu enviei, mas ela recebeu?". Registramos
-- timestamps de quando o e-mail saiu do relay — não é prova de leitura, mas
-- é o mesmo padrão de "delivered" do WhatsApp/e-mail corporativo, e é
-- muito mais do que hoje (nada).
--
-- Backfill: linhas antigas ficam NULL (não enviamos e-mail retroativo).
-- Idempotente pra re-run.

BEGIN;

ALTER TABLE proposal
    ADD COLUMN IF NOT EXISTS notified_author_at   timestamptz,
    ADD COLUMN IF NOT EXISTS notified_mandate_at  timestamptz;

COMMENT ON COLUMN proposal.notified_author_at IS
    '0.25.0-fediverso: quando o e-mail de confirmação saiu pro autor.';
COMMENT ON COLUMN proposal.notified_mandate_at IS
    '0.25.0-fediverso: quando o e-mail saiu pro gabinete (mandate.public_email).';

COMMIT;

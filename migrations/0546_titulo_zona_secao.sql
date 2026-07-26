-- Migration 0546 — Zona e Seção eleitorais no título de eleitor.
--
-- Complementa a 0105: além do número do título, o cidadão pode registrar a
-- ZONA e a SEÇÃO onde vota (constam no próprio título/e-Título). São dados
-- auxiliares — não entram na validação algorítmica dos dígitos e não mudam
-- o `titulo_status`; servem pra futura segmentação territorial fina (local
-- de votação) e pro cross-check TSE quando houver.
--
-- Formato TSE: zona com até 4 dígitos, seção com até 4 dígitos. Guardamos
-- como texto normalizado (só dígitos).
--
-- Idempotente: rerun-safe.

BEGIN;

ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS titulo_zona text
        CHECK (titulo_zona IS NULL OR titulo_zona ~ '^[0-9]{1,4}$'),
    ADD COLUMN IF NOT EXISTS titulo_secao text
        CHECK (titulo_secao IS NULL OR titulo_secao ~ '^[0-9]{1,4}$');

COMMENT ON COLUMN citizen.titulo_zona IS
    '0546: zona eleitoral (até 4 dígitos) declarada pelo cidadão — auxiliar, não valida o título.';
COMMENT ON COLUMN citizen.titulo_secao IS
    '0546: seção eleitoral (até 4 dígitos) declarada pelo cidadão — auxiliar, não valida o título.';

COMMIT;

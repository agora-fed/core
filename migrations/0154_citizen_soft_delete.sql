-- Migration 0154 — soft delete de citizen (LGPD art. 18 VI).
--
-- Excluir conta hard-delete quebra FKs cruzados (proposta autor=citizen,
-- sla mandate_binding, votos, comentários). Solução LGPD-compliant:
--   1. `deleted_at` timestamp na citizen.
--   2. Endpoints que listam citizens filtram `WHERE deleted_at IS NULL`.
--   3. Dados sensíveis (email, cpf) são limpos no delete; ID + handle
--      opaco ficam pra manter referências históricas de accountability.
--
-- LGPD art. 16: dados podem ser retidos quando necessários pro
-- exercício de direitos em processo judicial ou pra atender obrigação
-- legal. Registros públicos de votação/proposta se enquadram (interesse
-- público em accountability histórica de mandato).

BEGIN;

ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS deleted_at timestamptz;

CREATE INDEX IF NOT EXISTS citizen_active_idx
    ON citizen (org_id)
    WHERE deleted_at IS NULL;

COMMENT ON COLUMN citizen.deleted_at IS
    '0.26.0-fase-F: soft-delete LGPD. Quando setado, PII é limpa + login barrado.';

COMMIT;

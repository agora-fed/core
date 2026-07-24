-- Migration 0533 — escopo territorial (UF) opcional em debates (0.48.0, Fase 3.1).
--
-- Debate era org-global e chapado: o cidadão não achava o que é RELEVANTE pra ele
-- (risco de retenção apontado no plano — "sem 'meu estado' não engaja"). Uma UF
-- opcional deixa o debate ser nacional (NULL) OU do estado X, e a lista filtra por
-- "meu estado". Mínimo: só UF (não município), filtro client-side.

BEGIN;

ALTER TABLE debate
    ADD COLUMN uf text
    CONSTRAINT debate_uf_format CHECK (uf IS NULL OR uf ~ '^[A-Z]{2}$');

COMMENT ON COLUMN debate.uf IS
    '0.48.0: UF opcional de escopo territorial do debate (NULL = nacional).';

COMMIT;

-- Migration 0528 — soft-hide para o super-admin (SOCRATES).
--
-- "Excluir" no console admin OCULTA por padrão (reversível, preserva o registro
-- — coerente com "o silêncio é permanente"); um segundo botão faz o hard-delete
-- em cascata (irreversível) quando o admin realmente quer destruir.
--
-- Aqui só o soft-hide: `hidden_at NULL` = visível. As leituras públicas passam
-- a filtrar `hidden_at IS NULL`. O hard-delete é feito por DELETE em cascata no
-- código (não precisa de coluna).

BEGIN;

ALTER TABLE mandate  ADD COLUMN hidden_at timestamptz;
ALTER TABLE proposal ADD COLUMN hidden_at timestamptz;

-- Índices parciais: as leituras públicas filtram os NÃO-ocultos (o caso comum).
CREATE INDEX mandate_visible_idx  ON mandate  (org_id) WHERE hidden_at IS NULL;
CREATE INDEX proposal_visible_idx ON proposal (org_id) WHERE hidden_at IS NULL;

COMMENT ON COLUMN mandate.hidden_at IS
    '0.40.0: quando != NULL, o mandato foi ocultado por um admin (some das leituras públicas).';
COMMENT ON COLUMN proposal.hidden_at IS
    '0.40.0: quando != NULL, a proposta foi ocultada por um admin.';

COMMIT;

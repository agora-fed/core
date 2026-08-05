-- 0538_sla_per_gabinete — one SLA clock PER CABINET (phase 2 of 0537).
--
-- With multi-recipient proposals, the formal demand must hold for EVERY
-- cabinet: each recipient mandate gets its own SLA (deadline, D0/D+1/D+2
-- warning ladder and its own public record of silence). The idempotent
-- uniqueness of consuming `proposals.threshold.crossed` moves from
-- (proposal, cluster) to (proposal, cluster, mandate).
--
-- Existing rows stay valid (all have mandate_id NOT NULL); the new
-- constraint is strictly more permissive per proposal and equally strict
-- per cabinet. Idempotent on re-run.

BEGIN;

ALTER TABLE consequence_sla
    DROP CONSTRAINT IF EXISTS consequence_sla_proposal_id_cluster_id_key;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'consequence_sla_proposal_cluster_mandate_key'
    ) THEN
        ALTER TABLE consequence_sla
            ADD CONSTRAINT consequence_sla_proposal_cluster_mandate_key
            UNIQUE (proposal_id, cluster_id, mandate_id);
    END IF;
END $$;

COMMENT ON CONSTRAINT consequence_sla_proposal_cluster_mandate_key ON consequence_sla IS
    '0538: idempotência do threshold.crossed por gabinete — um SLA por destinatário da proposta.';

COMMIT;

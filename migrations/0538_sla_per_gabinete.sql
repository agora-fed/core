-- 0538_sla_per_gabinete — um relógio de SLA POR GABINETE (fase 2 do 0537).
--
-- Com propostas multi-destinatário, a cobrança formal deve valer para TODOS os
-- gabinetes: cada mandato destinatário ganha seu próprio SLA (prazo, escada de
-- avisos D0/D+1/D+2 e registro público de silêncio individuais). A unicidade
-- idempotente do consumo de `proposals.threshold.crossed` passa de
-- (proposal, cluster) para (proposal, cluster, mandate).
--
-- Linhas existentes continuam válidas (todas têm mandate_id NOT NULL); a nova
-- constraint é estritamente mais permissiva por proposta e igualmente estrita
-- por gabinete. Idempotente pra re-run.

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

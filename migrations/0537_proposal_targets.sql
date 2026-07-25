-- 0537_proposal_targets — múltiplos destinatários por proposta (mesma esfera).
--
-- Uma proposta pode ser dirigida a VÁRIOS gabinetes de uma vez (ex.: um grupo de
-- deputados federais), desde que todos sejam da MESMA esfera federativa
-- (federal | estadual | municipal — validado na camada de serviço, coluna
-- mandate.sphere da 0203). O apoio continua único: a proposta é uma só; o que
-- multiplica é a entrega e o recibo por gabinete.
--
-- `proposal.mandate_id` permanece como o destinatário PRINCIPAL (dirige o loop
-- de consequência/SLA e mantém compat com eventos, federação e clientes velhos).
-- Esta tabela guarda o conjunto COMPLETO de destinatários, principal incluído.
--
-- FKs: `proposal` é intra-crate (proposals); `mandate` é tabela de identidade
-- core — ambos permitidos pela regra do REGISTRY.md.

BEGIN;

CREATE TABLE IF NOT EXISTS proposal_target (
    proposal_id  uuid NOT NULL REFERENCES proposal(id),
    mandate_id   uuid NOT NULL REFERENCES mandate(id),
    -- Recibo de entrega por gabinete (e-mail saiu do relay) — mesmo padrão
    -- "delivered" da 0303, agora por destinatário.
    notified_at  timestamptz,
    created_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (proposal_id, mandate_id)
);

-- O painel do político lista "propostas dirigidas a mim" — consulta por mandato.
CREATE INDEX IF NOT EXISTS proposal_target_mandate_idx
    ON proposal_target (mandate_id, proposal_id);

-- Backfill: toda proposta existente vira 1 linha (seu alvo principal),
-- preservando o recibo legado. Idempotente pra re-run.
INSERT INTO proposal_target (proposal_id, mandate_id, notified_at, created_at)
SELECT id, mandate_id, notified_mandate_at, created_at
  FROM proposal
ON CONFLICT (proposal_id, mandate_id) DO NOTHING;

COMMENT ON TABLE proposal_target IS
    'Destinatários da proposta (0537): conjunto completo de gabinetes, principal incluído; recibo de entrega por gabinete.';

-- Em prod as migrations rodam como `postgres`, mas o gateway conecta como `dsoc`
-- (gotcha documentado em deployment-workflow: 0106/0107/0111/0151).
ALTER TABLE proposal_target OWNER TO dsoc;

COMMIT;

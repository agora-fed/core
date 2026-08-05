-- 0666_op_participatory_budget.sql — Participatory budgeting (a MANDATE pilot, D8.3).
--
-- The leap from "measuring anger" to "exercising power": the base decides where a REAL slice of funding goes.
-- A viable pilot = the AMENDMENT funds of an allied mandate (a council member/deputy), NOT the city hall —
-- it avoids the long institutional cycle. Reference: Porto Alegre's Participatory Budget.
-- Honest copy on every surface: "pilot — the mandate's amendment funds".
--
-- What separates PB from "one more poll" is the ACCOUNTABILITY: `op_item.execution_status`
-- closes the loop (planned → in progress → completed / not executed) after the vote.
--
-- Cycle of a round (`op_round.phase`):
--   propostas → votacao → resultado → execucao
--
-- Escopo territorial opcional (uf / municipio_ibge) espelha `citizen.municipio_ibge`/`uf` — a UI
-- may restrict who votes to the mandate's territory. The operator's gate comes from
-- `mandate_identity_binding` (the same criterion as campanha.rs / me_mandate_crm.rs).
--
-- OWNER: ALTER TABLE op_round   OWNER TO dsoc;
-- OWNER: ALTER TABLE op_item    OWNER TO dsoc;
-- OWNER: ALTER TABLE op_vote    OWNER TO dsoc;
--
-- Idempotente: rerun-safe.

BEGIN;

-- A participatory-budget round opened by a mandate, with a real funding cap.
CREATE TABLE IF NOT EXISTS op_round (
    id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    mandate_id     uuid NOT NULL REFERENCES mandate(id),
    title          text NOT NULL,
    budget_cents   bigint NOT NULL,                 -- verba da emenda em centavos (teto do ciclo)
    uf             text,                            -- escopo territorial opcional (sigla UF)
    municipio_ibge integer,                         -- escopo territorial opcional (código IBGE)
    phase          text NOT NULL DEFAULT 'propostas'
        CHECK (phase IN ('propostas', 'votacao', 'resultado', 'execucao')),
    created_at     timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS op_round_mandate_idx ON op_round (mandate_id, created_at DESC);

-- An item/proposal submitted to a round. `estimated_cents` is the estimated cost (it enters the
-- "fits the budget" computation). `execution_status` is the post-vote accountability.
CREATE TABLE IF NOT EXISTS op_item (
    id                uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    round_id          uuid NOT NULL REFERENCES op_round(id) ON DELETE CASCADE,
    author_citizen_id uuid REFERENCES citizen(id),  -- NULL = item do gabinete / anônimo
    title             text NOT NULL,
    description       text NOT NULL DEFAULT '',
    estimated_cents   bigint,                        -- custo estimado (NULL = sem estimativa)
    execution_status  text
        CHECK (execution_status IN ('previsto', 'em_andamento', 'concluido', 'nao_executado')),
    created_at        timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS op_item_round_idx ON op_item (round_id);

-- One vote: 1 per citizen PER ROUND (the PK guarantees uniqueness). Switching item = an upsert.
CREATE TABLE IF NOT EXISTS op_vote (
    round_id   uuid NOT NULL REFERENCES op_round(id) ON DELETE CASCADE,
    item_id    uuid NOT NULL REFERENCES op_item(id) ON DELETE CASCADE,
    citizen_id uuid NOT NULL REFERENCES citizen(id),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (round_id, citizen_id)
);
CREATE INDEX IF NOT EXISTS op_vote_item_idx ON op_vote (item_id);

COMMENT ON TABLE op_round IS
    '0666 (D8.3): rodada de orçamento participativo — piloto de verba de emenda de um mandato.';
COMMENT ON TABLE op_item IS
    '0666 (D8.3): item/proposta de uma rodada de OP, com custo estimado e status de execução.';
COMMENT ON TABLE op_vote IS
    '0666 (D8.3): voto de OP — 1 por cidadão por rodada (PK round_id+citizen_id).';

COMMIT;

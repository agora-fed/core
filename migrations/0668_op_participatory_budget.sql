-- 0666_op_participatory_budget.sql — Orçamento participativo (piloto de MANDATO, D8.3).
--
-- O salto de "medir raiva" para "exercer poder": a base decide onde vai uma fatia REAL de verba.
-- Piloto viável = verba de EMENDA de um mandato aliado (um vereador/deputado), NÃO a prefeitura —
-- evita o ciclo institucional longo. Referência: Orçamento Participativo de Porto Alegre.
-- Copy honesta em toda superfície: "piloto — verba de emenda do mandato".
--
-- O que separa OP de "mais uma enquete" é a PRESTAÇÃO DE CONTAS: `op_item.execution_status`
-- fecha o loop (previsto → em_andamento → concluído / não executado) depois da votação.
--
-- Ciclo de uma rodada (`op_round.phase`):
--   propostas → votacao → resultado → execucao
--
-- Escopo territorial opcional (uf / municipio_ibge) espelha `citizen.municipio_ibge`/`uf` — a UI
-- pode restringir quem vota ao território do mandato. O gate do operador vem de
-- `mandate_identity_binding` (mesmo critério de campanha.rs / me_mandate_crm.rs).
--
-- OWNER: ALTER TABLE op_round   OWNER TO dsoc;
-- OWNER: ALTER TABLE op_item    OWNER TO dsoc;
-- OWNER: ALTER TABLE op_vote    OWNER TO dsoc;
--
-- Idempotente: rerun-safe.

BEGIN;

-- Uma rodada de orçamento participativo aberta por um mandato, com um teto de verba real.
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

-- Um item/proposta submetido a uma rodada. `estimated_cents` é o custo estimado (entra no
-- cálculo do "cabe no orçamento"). `execution_status` é a prestação de contas pós-votação.
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

-- Um voto: 1 por cidadão POR RODADA (a PK garante a unicidade). Trocar de item = upsert.
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

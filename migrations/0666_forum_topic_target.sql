-- 0666_forum_topic_target — direcionamento OPCIONAL do tópico de fórum a mandato(s) (B1).
--
-- Fusão Propor ≡ Fórum: o tópico de fórum passa a ser a ÚNICA unidade
-- deliberativa e ganha o poder que a "proposta" tinha — direcionar uma demanda
-- a gabinete(s) ESPECÍFICO(s). Uma porta, uma régua: o mesmo placar por pontos e
-- o mesmo patamar proporcional (dsoc_core::proportional_threshold, piso 10) do
-- fórum; o alvo só troca PARA ONDE o encaminhamento vai quando o patamar cruza.
--
-- Tópico SEM alvo → contato curado da seção (comportamento atual, ADR-0019/D3).
-- Tópico COM alvo → encaminha ao public_email de cada mandato alcançável (o
-- placeholder @parlamento.democracia.social.br é filtrado no serviço — Tier 0,
-- igual proposal_delivery: nunca entregamos num inbox morto nem carimbamos SLA).
--
-- FKs: forum_topic é intra-arquivo (forums, 0540); mandate é tabela de
-- identidade core — ambos permitidos pela regra do REGISTRY.md.
--
-- OWNER: aplicar em prod com ALTER TABLE ... OWNER TO dsoc — em prod as migrations
-- rodam como `postgres`, mas o gateway conecta como `dsoc` (gotcha documentado nas
-- 0106/0537/0540). Índice novo em forum_dispatch nasce no schema da tabela dsoc.

BEGIN;

CREATE TABLE IF NOT EXISTS forum_topic_target (
    topic_id    uuid NOT NULL REFERENCES forum_topic(id),
    mandate_id  uuid NOT NULL REFERENCES mandate(id),
    created_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (topic_id, mandate_id)
);

-- O painel do gabinete lista "tópicos dirigidos a mim" — consulta por mandato.
CREATE INDEX IF NOT EXISTS forum_topic_target_mandate_idx
    ON forum_topic_target (mandate_id, topic_id);

COMMENT ON TABLE forum_topic_target IS
    'Alvos opcionais de um tópico de fórum (B1): gabinete(s) que recebem o encaminhamento quando o patamar cruza; vazio = contato curado da seção.';

-- Recibo por ALVO: um tópico direcionado a N gabinetes registra N recibos no
-- MESMO patamar. Precisamos discriminar o destinatário no UNIQUE, senão o
-- ON CONFLICT (topic_id, threshold) colapsaria todos num só recibo. mandate_id
-- NULL = envio à seção (comportamento atual); NULLS NOT DISTINCT preserva o
-- "1x por patamar" da seção (PG15+, mesmo recurso já usado na 0540).
ALTER TABLE forum_dispatch
    ADD COLUMN IF NOT EXISTS mandate_id uuid REFERENCES mandate(id);
ALTER TABLE forum_dispatch
    DROP CONSTRAINT IF EXISTS forum_dispatch_topic_id_threshold_key;
CREATE UNIQUE INDEX IF NOT EXISTS forum_dispatch_topic_threshold_mandate_key
    ON forum_dispatch (topic_id, threshold, mandate_id) NULLS NOT DISTINCT;

COMMENT ON COLUMN forum_dispatch.mandate_id IS
    'Gabinete alvo do recibo (B1); NULL = envio ao contato curado da seção.';

ALTER TABLE forum_topic_target OWNER TO dsoc;

COMMIT;

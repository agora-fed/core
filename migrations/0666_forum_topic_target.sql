-- 0666_forum_topic_target — OPTIONAL direction of a forum topic at mandate(s) (B1).
--
-- The Propose ≡ Forum merge: the forum topic becomes the ONLY deliberative
-- unit and gains the power the "proposal" had — directing a demand
-- at SPECIFIC office(s). One door, one yardstick: the same points scoreboard and
-- o mesmo patamar proporcional (dsoc_core::proportional_threshold, piso 10) do
-- forum; the target only changes WHERE the dispatch goes once the threshold crosses.
--
-- A topic WITHOUT a target → the section's curated contact (current behaviour, ADR-0019/D3).
-- A topic WITH a target → dispatches to each reachable mandate's public_email (the
-- @parlamento.democracia.social.br placeholder is filtered in the service — Tier 0,
-- igual proposal_delivery: nunca entregamos num inbox morto nem carimbamos SLA).
--
-- FKs: forum_topic is intra-file (forums, 0540); mandate is a core identity
-- table — both allowed by the REGISTRY.md rule.
--
-- OWNER: apply in production with ALTER TABLE ... OWNER TO dsoc — in production the migrations
-- run as `postgres`, but the gateway connects as `dsoc` (the gotcha documented in
-- 0106/0537/0540). The new index on forum_dispatch is born in the dsoc table's schema.

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

-- Receipt per TARGET: a topic directed at N offices records N receipts at the
-- SAME threshold. We must discriminate the recipient in the UNIQUE, otherwise
-- ON CONFLICT (topic_id, threshold) would collapse them all into one receipt. mandate_id
-- NULL = a dispatch to the section (current behaviour); NULLS NOT DISTINCT preserves the
-- section's "once per threshold" (PG15+, the same feature already used in 0540).
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

-- Migration 0532 — enquetes dirigidas do grupo de campanha (0.45.0, Fase 3.4).
--
-- O grupo de campanha (0527) era só broadcast: o político PUBLICA e o eleitor LÊ.
-- Falta a mão-dupla — o político perguntar à sua base e ouvir a resposta. Aqui
-- ele abre uma "enquete rápida" (uma pergunta, concordo/neutro/discordo) dirigida
-- ao seu grupo; o cidadão logado responde e o resultado agrega ao vivo. Mesmo
-- motor de agregação das consultas (0531), mas com DONO (o mandato do grupo) —
-- é o canal proativo campanha→eleitor que o plano pede na 3.4.

BEGIN;

CREATE TABLE campaign_group_poll (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    group_id    uuid NOT NULL REFERENCES campaign_group(id) ON DELETE CASCADE,
    question    text NOT NULL,
    status      text NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'closed')),
    created_at  timestamptz NOT NULL DEFAULT now(),
    closed_at   timestamptz
);
CREATE INDEX campaign_group_poll_group_idx
    ON campaign_group_poll (group_id, created_at DESC, id DESC);

CREATE TABLE campaign_group_poll_response (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    poll_id     uuid NOT NULL REFERENCES campaign_group_poll(id) ON DELETE CASCADE,
    citizen_id  uuid NOT NULL REFERENCES citizen(id),
    answer      text NOT NULL CHECK (answer IN ('concordo', 'neutro', 'discordo')),
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    -- Uma resposta por cidadão por enquete (o upsert atualiza).
    UNIQUE (poll_id, citizen_id)
);
CREATE INDEX campaign_group_poll_response_poll_idx
    ON campaign_group_poll_response (poll_id, answer);

COMMENT ON TABLE campaign_group_poll IS
    '0.45.0: enquete rápida dirigida pelo dono do grupo de campanha à sua base.';
COMMENT ON TABLE campaign_group_poll_response IS
    '0.45.0: resposta de um cidadão a uma enquete de campanha (concordo/neutro/discordo).';

-- O pod do gateway conecta como dsoc.
ALTER TABLE campaign_group_poll OWNER TO dsoc;
ALTER TABLE campaign_group_poll_response OWNER TO dsoc;

COMMIT;

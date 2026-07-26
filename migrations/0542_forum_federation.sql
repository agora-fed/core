-- 0542_forum_federation — F4: fórum como ator Group seguível do fediverso.
--
-- Handle federado SEM prefixo (decisão 2026-07-26): segmentos do caminho
-- INVERTIDOS unidos por ponto — @ministerio-educacao@, @sp@, @saude.sp@
-- (secretaria estadual), @santos.sp@ (cidade), @saude.santos.sp@ (seção
-- municipal), @ccj.senado@ (comissão). Chaves do Group ficam nas colunas
-- public/private_key_pem da 0540 (geração preguiçosa no 1º uso).

BEGIN;

-- Seguidores remotos de um fórum (Follow → Accept assinado pelo Group).
CREATE TABLE IF NOT EXISTS forum_follower (
    forum_id          uuid NOT NULL REFERENCES forum(id),
    remote_actor_url  text NOT NULL,
    remote_inbox_url  text NOT NULL,
    accepted_at       timestamptz,
    created_at        timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (forum_id, remote_actor_url)
);

-- Fan-out do Announce de cada tópico aos seguidores — 1 linha por (tópico, inbox),
-- varrida pelo worker (mesmo padrão do forum_dispatch: envio nunca duplica).
CREATE TABLE IF NOT EXISTS forum_announce_delivery (
    id               uuid PRIMARY KEY,
    topic_id         uuid NOT NULL REFERENCES forum_topic(id),
    recipient_inbox  text NOT NULL,
    attempts         integer NOT NULL DEFAULT 0,
    sent_at          timestamptz,
    created_at       timestamptz NOT NULL DEFAULT now(),
    UNIQUE (topic_id, recipient_inbox)
);
CREATE INDEX IF NOT EXISTS forum_announce_pending_idx
    ON forum_announce_delivery (sent_at, attempts) WHERE sent_at IS NULL;

COMMENT ON TABLE forum_follower IS 'Seguidores ActivityPub de um fórum (ator Group, 0542).';
COMMENT ON TABLE forum_announce_delivery IS 'Fan-out de Announce dos tópicos aos seguidores do fórum (0542).';

-- Prod aplica migrations como postgres; o gateway conecta como dsoc.
ALTER TABLE forum_follower OWNER TO dsoc;
ALTER TABLE forum_announce_delivery OWNER TO dsoc;

COMMIT;

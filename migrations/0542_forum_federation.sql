-- 0542_forum_federation — F4: the forum as a followable fediverse Group actor.
--
-- Federated handle WITHOUT a prefix (decision 2026-07-26): path segments
-- REVERSED and joined by dots — @ministerio-educacao@, @sp@, @saude.sp@
-- (state secretariat), @santos.sp@ (city), @saude.santos.sp@ (municipal
-- section), @ccj.senado@ (committee). The Group's keys live in the
-- public/private_key_pem columns from 0540 (generated lazily on first use).

BEGIN;

-- Remote followers of a forum (Follow → Accept signed by the Group).
CREATE TABLE IF NOT EXISTS forum_follower (
    forum_id          uuid NOT NULL REFERENCES forum(id),
    remote_actor_url  text NOT NULL,
    remote_inbox_url  text NOT NULL,
    accepted_at       timestamptz,
    created_at        timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (forum_id, remote_actor_url)
);

-- Fan-out of each topic's Announce to the followers — 1 row per (topic, inbox),
-- swept by the worker (same pattern as forum_dispatch: delivery never duplicates).
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

-- Prod applies migrations as postgres; the gateway connects as dsoc.
ALTER TABLE forum_follower OWNER TO dsoc;
ALTER TABLE forum_announce_delivery OWNER TO dsoc;

COMMIT;

-- 0541_forum_moderators — per-forum moderators (F3 configuration).
--
-- Local citizens designated by the platform admin to moderate a specific
-- forum (hide topics/comments; future curation actions).
-- FKs: forum (intra-crate 0540) + citizen (core) — both allowed.

BEGIN;

CREATE TABLE IF NOT EXISTS forum_moderator (
    forum_id    uuid NOT NULL REFERENCES forum(id),
    citizen_id  uuid NOT NULL REFERENCES citizen(id),
    created_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (forum_id, citizen_id)
);

COMMENT ON TABLE forum_moderator IS
    'Moderadores designados por fórum (0541) — gestão via painel admin.';

-- Prod applies migrations as postgres; the gateway connects as dsoc.
ALTER TABLE forum_moderator OWNER TO dsoc;

COMMIT;

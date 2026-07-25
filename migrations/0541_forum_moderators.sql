-- 0541_forum_moderators — moderadores por fórum (config F3).
--
-- Cidadãos locais designados pelo admin da plataforma para moderar um fórum
-- específico (ocultar tópicos/comentários; futuras ações de curadoria).
-- FKs: forum (intra-crate 0540) + citizen (core) — permitidas.

BEGIN;

CREATE TABLE IF NOT EXISTS forum_moderator (
    forum_id    uuid NOT NULL REFERENCES forum(id),
    citizen_id  uuid NOT NULL REFERENCES citizen(id),
    created_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (forum_id, citizen_id)
);

COMMENT ON TABLE forum_moderator IS
    'Moderadores designados por fórum (0541) — gestão via painel admin.';

-- Prod aplica migrations como postgres; o gateway conecta como dsoc.
ALTER TABLE forum_moderator OWNER TO dsoc;

COMMIT;

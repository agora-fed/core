-- 0545_forum_comment_votes — votos nos argumentos (estilo StackOverflow).
--
-- "As vezes o comentário é mais relevante que o próprio tópico": cada
-- argumento de um tópico de fórum aceita posição a favor/contra/ponderação
-- de cidadãos LOCAIS (FK citizen = regra estrutural, como no voto do tópico).
-- Uma posição por cidadão por argumento (mutável). Contadores materializados
-- no comentário; dentro de cada coluna a UI ordena por saldo (favor - contra).
-- Votos em argumentos são interações locais CONTÁVEIS (entram nos patamares).
--
-- Idempotente: rerun-safe.

BEGIN;

CREATE TABLE IF NOT EXISTS forum_comment_vote (
    comment_id  uuid NOT NULL REFERENCES forum_topic_comment(id),
    citizen_id  uuid NOT NULL REFERENCES citizen(id),
    stance      text NOT NULL CHECK (stance IN ('favor', 'contra', 'ponderacao')),
    created_at  timestamptz NOT NULL,
    PRIMARY KEY (comment_id, citizen_id)
);

ALTER TABLE forum_topic_comment
    ADD COLUMN IF NOT EXISTS favor_count      bigint NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS contra_count     bigint NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS ponderacao_count bigint NOT NULL DEFAULT 0;

COMMENT ON TABLE forum_comment_vote IS
    '0545: posição de cidadão local num argumento — favor|contra|ponderacao, 1 por par.';

-- Prod aplica migrations como postgres; o gateway conecta como dsoc.
ALTER TABLE forum_comment_vote OWNER TO dsoc;

COMMIT;

-- 0665_forum_karma_drop_ponderacao.sql — F1 do placar por pontos + karma (ÁGORA, ADR-0019).
--
-- Diretriz do Marcos (2026-07-28): novo placar de deliberação estilo StackOverflow/Odoo.
-- Esta fatia (F1) faz só schema + limpeza de dados; a fórmula de pontos e o accrual de karma
-- vêm na F2 (backend). Aqui:
--   1. APAGA todo dado com stance 'ponderacao' (voto de tópico, comentário e voto-em-comentário) —
--      decisão do Marcos (ponderação eliminada por completo).
--   2. Reduz o CHECK de stance a ('favor','contra') nas 3 tabelas.
--   3. Adiciona `citizen.karma` (reputação estilo SO; alimentada na F2).
--
-- `ponderacao_count` (forum_topic / forum_topic_comment) fica como VESTIGIAL (sempre 0 agora) e é
-- dropada na F2, junto com a reescrita do recount que a alimentava. Idempotente: rerun-safe.

BEGIN;

-- 1. Apaga dados de ponderação (ordem respeitando FKs).
DELETE FROM forum_comment_vote WHERE stance = 'ponderacao';
DELETE FROM forum_comment_vote
 WHERE comment_id IN (SELECT id FROM forum_topic_comment WHERE stance = 'ponderacao');
DELETE FROM forum_topic_comment WHERE stance = 'ponderacao';
DELETE FROM forum_topic_vote    WHERE stance = 'ponderacao';

-- Zera os contadores vestigiais (a F2 recomputa tudo com a fórmula nova).
UPDATE forum_topic         SET ponderacao_count = 0 WHERE ponderacao_count <> 0;
UPDATE forum_topic_comment SET ponderacao_count = 0 WHERE ponderacao_count <> 0;

-- 2. CHECK de stance → só favor/contra.
ALTER TABLE forum_topic_vote    DROP CONSTRAINT IF EXISTS forum_topic_vote_stance_check;
ALTER TABLE forum_topic_vote    ADD  CONSTRAINT forum_topic_vote_stance_check
     CHECK (stance = ANY (ARRAY['favor'::text, 'contra'::text]));

ALTER TABLE forum_topic_comment DROP CONSTRAINT IF EXISTS forum_topic_comment_stance_check;
ALTER TABLE forum_topic_comment ADD  CONSTRAINT forum_topic_comment_stance_check
     CHECK (stance IS NULL OR stance = ANY (ARRAY['favor'::text, 'contra'::text]));

ALTER TABLE forum_comment_vote  DROP CONSTRAINT IF EXISTS forum_comment_vote_stance_check;
ALTER TABLE forum_comment_vote  ADD  CONSTRAINT forum_comment_vote_stance_check
     CHECK (stance = ANY (ARRAY['favor'::text, 'contra'::text]));

-- 3. Karma do cidadão (reputação estilo SO; F2 preenche via votos em comentários).
ALTER TABLE citizen ADD COLUMN IF NOT EXISTS karma integer NOT NULL DEFAULT 0;

COMMENT ON COLUMN citizen.karma IS
    '0665 (ADR-0019): reputação estilo SO. +10 por voto positivo em comentário do usuário, -2 por negativo.';

COMMIT;

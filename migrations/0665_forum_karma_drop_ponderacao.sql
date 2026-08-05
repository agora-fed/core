-- 0665_forum_karma_drop_ponderacao.sql — F1 of the points scoreboard + karma (AGORA, ADR-0019).
--
-- Marcos' directive (2026-07-28): a new StackOverflow/Odoo-style deliberation scoreboard.
-- This slice (F1) does schema + data cleanup only; the points formula and the karma accrual
-- come in F2 (backend). Here:
--   1. DELETES every datum with the 'ponderacao' stance (topic vote, comment and comment vote) —
--      Marcos' decision (the neutral option removed entirely).
--   2. Narrows the stance CHECK to ('favor','contra') on the 3 tables.
--   3. Adds `citizen.karma` (SO-style reputation; populated in F2).
--
-- `ponderacao_count` (forum_topic / forum_topic_comment) stays VESTIGIAL (always 0 now) and is
-- dropped in F2, along with rewriting the recount that fed it. Idempotent: rerun-safe.

BEGIN;

-- 1. Delete the neutral-stance data (order respecting the FKs).
DELETE FROM forum_comment_vote WHERE stance = 'ponderacao';
DELETE FROM forum_comment_vote
 WHERE comment_id IN (SELECT id FROM forum_topic_comment WHERE stance = 'ponderacao');
DELETE FROM forum_topic_comment WHERE stance = 'ponderacao';
DELETE FROM forum_topic_vote    WHERE stance = 'ponderacao';

-- Zero the vestigial counters (F2 recomputes everything with the new formula).
UPDATE forum_topic         SET ponderacao_count = 0 WHERE ponderacao_count <> 0;
UPDATE forum_topic_comment SET ponderacao_count = 0 WHERE ponderacao_count <> 0;

-- 2. Stance CHECK → favor/contra only.
ALTER TABLE forum_topic_vote    DROP CONSTRAINT IF EXISTS forum_topic_vote_stance_check;
ALTER TABLE forum_topic_vote    ADD  CONSTRAINT forum_topic_vote_stance_check
     CHECK (stance = ANY (ARRAY['favor'::text, 'contra'::text]));

ALTER TABLE forum_topic_comment DROP CONSTRAINT IF EXISTS forum_topic_comment_stance_check;
ALTER TABLE forum_topic_comment ADD  CONSTRAINT forum_topic_comment_stance_check
     CHECK (stance IS NULL OR stance = ANY (ARRAY['favor'::text, 'contra'::text]));

ALTER TABLE forum_comment_vote  DROP CONSTRAINT IF EXISTS forum_comment_vote_stance_check;
ALTER TABLE forum_comment_vote  ADD  CONSTRAINT forum_comment_vote_stance_check
     CHECK (stance = ANY (ARRAY['favor'::text, 'contra'::text]));

-- 3. The citizen's karma (SO-style reputation; F2 fills it via comment votes).
ALTER TABLE citizen ADD COLUMN IF NOT EXISTS karma integer NOT NULL DEFAULT 0;

COMMENT ON COLUMN citizen.karma IS
    '0665 (ADR-0019): reputação estilo SO. +10 por voto positivo em comentário do usuário, -2 por negativo.';

COMMIT;

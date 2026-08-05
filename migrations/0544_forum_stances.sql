-- 0544_forum_stances — the DEBATE→FORUM merge (issue #19, 2026-07-26).
--
-- The Debates × Forums duality ends: the forum topic adopts the debate's
-- consolidated functional model — participation is a STANCE
-- ('favor' | 'contra' | 'ponderacao'), one per citizen (changeable), and a
-- comment may carry the author's stance at the moment of the argument.
-- Per-stance counters are materialized on the topic (for list display).
--
-- score stays = favor - contra (the "hot" ordering is preserved);
-- countable interactions stay votes + LOCAL comments (thresholds).
--
-- Idempotente: rerun-safe.

BEGIN;

-- The ±1 vote becomes a stance. Backfill: +1 → favor, -1 → contra.
ALTER TABLE forum_topic_vote ADD COLUMN IF NOT EXISTS stance text;
UPDATE forum_topic_vote
   SET stance = CASE WHEN value = 1 THEN 'favor' ELSE 'contra' END
 WHERE stance IS NULL;
ALTER TABLE forum_topic_vote ALTER COLUMN stance SET NOT NULL;
ALTER TABLE forum_topic_vote
    DROP CONSTRAINT IF EXISTS forum_topic_vote_stance_check;
ALTER TABLE forum_topic_vote
    ADD CONSTRAINT forum_topic_vote_stance_check
    CHECK (stance IN ('favor', 'contra', 'ponderacao'));
ALTER TABLE forum_topic_vote DROP COLUMN IF EXISTS value;

-- A comment may carry the author's stance (NULL = no declared stance;
-- federated comments never have one — they do not vote).
ALTER TABLE forum_topic_comment ADD COLUMN IF NOT EXISTS stance text;
ALTER TABLE forum_topic_comment
    DROP CONSTRAINT IF EXISTS forum_topic_comment_stance_check;
ALTER TABLE forum_topic_comment
    ADD CONSTRAINT forum_topic_comment_stance_check
    CHECK (stance IS NULL OR stance IN ('favor', 'contra', 'ponderacao'));

-- Per-stance counters, materialized (recomputed under the topic's row lock).
ALTER TABLE forum_topic
    ADD COLUMN IF NOT EXISTS favor_count      bigint NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS contra_count     bigint NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS ponderacao_count bigint NOT NULL DEFAULT 0;

-- Backfill the counters + score from the existing votes.
UPDATE forum_topic t
   SET favor_count      = s.f,
       contra_count     = s.c,
       ponderacao_count = s.p,
       score            = s.f - s.c
  FROM (SELECT topic_id,
               COUNT(*) FILTER (WHERE stance = 'favor')      AS f,
               COUNT(*) FILTER (WHERE stance = 'contra')     AS c,
               COUNT(*) FILTER (WHERE stance = 'ponderacao') AS p
          FROM forum_topic_vote GROUP BY topic_id) s
 WHERE s.topic_id = t.id;

COMMENT ON COLUMN forum_topic_vote.stance IS
    '0544: posição do cidadão — favor | contra | ponderacao (fusão debates→fóruns).';
COMMENT ON COLUMN forum_topic_comment.stance IS
    '0544: posição declarada junto do argumento (NULL = sem posição / federado).';

COMMIT;

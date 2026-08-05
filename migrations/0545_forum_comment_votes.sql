-- 0545_forum_comment_votes — votes on arguments (StackOverflow style).
--
-- "Sometimes the comment matters more than the topic itself": every
-- argument under a forum topic accepts a for/against/qualifying position
-- from LOCAL citizens (FK citizen = structural rule, as in the topic vote).
-- One position per citizen per argument (mutable). Counters materialised
-- on the comment; within each column the UI sorts by balance (for - against).
-- Votes on arguments are COUNTABLE local interactions (they feed the thresholds).
--
-- Idempotent: rerun-safe.

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

-- Prod applies migrations as postgres; the gateway connects as dsoc.
ALTER TABLE forum_comment_vote OWNER TO dsoc;

COMMIT;

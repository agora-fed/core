-- 0678 — idempotency log for the Group/forum inbox (issue agora-fed/core#6).
--
-- The Person inbox has `federation_inbox_seen` (keyed by activity id +
-- receiving citizen). The Group inbox had NO idempotency at all, so a replayed
-- Follow/Undo re-ran the mutation. Forums are not citizens, so they need their
-- own log keyed by (activity_id, forum_id).

CREATE TABLE forum_inbox_seen (
    activity_id text NOT NULL,
    forum_id    uuid NOT NULL REFERENCES forum(id) ON DELETE CASCADE,
    seen_at     timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (activity_id, forum_id)
);

CREATE INDEX forum_inbox_seen_forum_idx ON forum_inbox_seen (forum_id, seen_at DESC);

COMMENT ON TABLE forum_inbox_seen IS
    'gateway: strict idempotency log for the Group/forum inbox (issue #6). Insert-before-act: a duplicate activity id is a no-op.';

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'dsoc') THEN
        ALTER TABLE forum_inbox_seen OWNER TO dsoc;
    END IF;
END $$;

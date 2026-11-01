-- Contributions from the fediverse (FEP-1b12). Remote authors never become login accounts:
-- topics point at one per-org system citizen and carry the remote actor alongside it, comments
-- already had remote_actor_url. A pending or rejected topic stays hidden through hidden_at, the
-- filter every reader already applies.

ALTER TABLE forum_topic
    ADD COLUMN remote_actor_url text,
    ADD COLUMN remote_handle    text,
    ADD COLUMN remote_activity  jsonb,
    ADD COLUMN moderation       text NOT NULL DEFAULT 'approved'
        CONSTRAINT forum_topic_moderation_check CHECK (moderation IN ('pending', 'approved', 'rejected'));
CREATE UNIQUE INDEX forum_topic_ap_object_uri_key ON forum_topic (ap_object_uri)
    WHERE ap_object_uri IS NOT NULL;
CREATE INDEX forum_topic_pending_idx ON forum_topic (created_at) WHERE moderation = 'pending';

ALTER TABLE forum_topic_comment
    ADD COLUMN ap_object_uri   text,
    ADD COLUMN remote_activity jsonb;
CREATE UNIQUE INDEX forum_topic_comment_ap_object_uri_key ON forum_topic_comment (ap_object_uri)
    WHERE ap_object_uri IS NOT NULL;
CREATE INDEX forum_topic_comment_pending_idx ON forum_topic_comment (created_at)
    WHERE moderation = 'pending';

-- Remote votes are shown beside local ones but never count for thresholds or score.
CREATE TABLE forum_remote_vote (
    target_kind text NOT NULL CHECK (target_kind IN ('topic', 'comment')),
    target_id   uuid NOT NULL,
    actor_url   text NOT NULL,
    stance      text NOT NULL CHECK (stance IN ('favor', 'contra')),
    activity_id text,
    created_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (target_kind, target_id, actor_url)
);
CREATE INDEX forum_remote_vote_actor_idx ON forum_remote_vote (actor_url);

CREATE TABLE forum_remote_actor_block (
    id         uuid PRIMARY KEY,
    actor_url  text NOT NULL UNIQUE,
    reason     text,
    created_by uuid REFERENCES citizen (id) ON DELETE SET NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

-- What a Group sends to its followers besides its own new topics.
CREATE TABLE forum_outbound_activity (
    id         uuid PRIMARY KEY,
    forum_id   uuid NOT NULL REFERENCES forum (id) ON DELETE CASCADE,
    dedupe_key text NOT NULL UNIQUE,
    activity   jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE forum_outbound_delivery (
    activity_id     uuid NOT NULL REFERENCES forum_outbound_activity (id) ON DELETE CASCADE,
    recipient_inbox text NOT NULL,
    attempts        integer NOT NULL DEFAULT 0,
    sent_at         timestamptz,
    PRIMARY KEY (activity_id, recipient_inbox)
);
CREATE INDEX forum_outbound_delivery_pending_idx ON forum_outbound_delivery (attempts)
    WHERE sent_at IS NULL;


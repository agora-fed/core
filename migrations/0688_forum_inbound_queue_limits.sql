-- Queue limits per server, and whether followers have already received a contribution, which
-- decides between relaying a Create and an Update.

ALTER TABLE forum_topic
    ADD COLUMN remote_host text,
    ADD COLUMN relayed     boolean NOT NULL DEFAULT false;
ALTER TABLE forum_topic_comment
    ADD COLUMN remote_host text,
    ADD COLUMN relayed     boolean NOT NULL DEFAULT false;

CREATE INDEX forum_topic_pending_host_idx ON forum_topic (remote_host) WHERE moderation = 'pending';
CREATE INDEX forum_topic_comment_pending_host_idx ON forum_topic_comment (remote_host)
    WHERE moderation = 'pending';

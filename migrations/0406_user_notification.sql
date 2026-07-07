-- 0406_user_notification — Mastodon parity fase 2 (0.18.0-beta).
-- User-facing notifications: "alguém curtiu / repostou / respondeu / te
-- mencionou / te seguiu". Distinct from `dsoc-notify` (the platform's
-- internal event-driven mail/SLA sink) — this table drives the in-app
-- /notificacoes feed for each citizen.
--
-- Design notes:
-- * `citizen_id` is the RECIPIENT (the local user who sees the item). The
--   TRIGGER (who acted) is captured as a set of denormalized snapshot
--   columns (`source_actor_url`, `source_handle`, `source_display_name`,
--   `source_avatar_url`) so remote actors that later go offline or rename
--   still render in the historical notification.
-- * `object_uri` points to the note the notification is about (a mention/
--   reply/favourite/reblog). NULL for `follow` events.
-- * `object_preview` is a plain-text snippet cached at insert-time so the
--   notification card renders without another lookup. Truncated to 240
--   chars.
-- * `UNIQUE (citizen_id, kind, source_actor_url, object_uri)` is the
--   idempotency key: a re-delivered inbox event does not create dupes;
--   multiple different actors CAN create multiple rows against the same
--   object (that's how Mastodon groups "3 pessoas curtiram" but stays
--   per-row for the counter).

CREATE TABLE user_notification (
    id                    uuid PRIMARY KEY,
    citizen_id            uuid NOT NULL REFERENCES citizen(id),
    kind                  text NOT NULL CHECK (kind IN
                              ('mention', 'reply', 'favourite', 'reblog', 'follow')),
    -- Snapshot of the acting actor (local or remote).
    source_actor_url      text,
    source_handle         text NOT NULL,
    source_display_name   text,
    source_avatar_url     text,
    -- The Note the event is about (NULL only for follow).
    object_uri            text,
    -- Cached plain-text preview (max 240 chars, sanitized upstream).
    object_preview        text,
    created_at            timestamptz NOT NULL,
    read_at               timestamptz,
    -- Idempotency: same (recipient, kind, source, object) is one row.
    UNIQUE (citizen_id, kind, source_actor_url, object_uri)
);

CREATE INDEX user_notification_citizen_created_idx
    ON user_notification (citizen_id, created_at DESC);

-- Unread badge query — partial index so the count stays cheap even when
-- read history piles up.
CREATE INDEX user_notification_unread_idx
    ON user_notification (citizen_id, created_at DESC)
    WHERE read_at IS NULL;

COMMENT ON TABLE user_notification IS
    'ADR-0010 W3 (0.18.0): per-citizen in-app notifications for mention/reply/favourite/reblog/follow.';

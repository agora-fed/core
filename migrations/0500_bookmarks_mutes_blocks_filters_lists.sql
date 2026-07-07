-- Migration 0500 — social-graph tables for Mastodon parity fase 2A.
--
-- Adds six small tables the front + Mastodon Client API need to reach full
-- parity with Ivory/Elk/Ice Cubes/Tusky's feature set:
--
--   note_bookmark        — a citizen bookmarks any note (local OR remote) by
--                          its stable object_uri. Rendered under /marcados.
--   actor_mute           — hides another actor's notes from the caller's feed
--                          + notifications, but still lets the target see us.
--   actor_block          — hard block: mute + refuse follow + drop inbound
--                          reactions from that actor.
--   content_filter       — regex-free substring filter for note content.
--                          Context is a small text[] of one or more of
--                          {'home','notifications','public','thread'} so a
--                          filter can scope where it applies (Mastodon-shape).
--   actor_list           — user-owned lists of accounts (Mastodon Lists API).
--   actor_list_member    — junction table (list_id, target_actor_url).
--
-- All targets that identify a remote OR local actor use `actor_url` (text) so
-- the same table works cross-instance without a join to citizen. Bookmarks
-- key by `object_uri` for the same reason.

BEGIN;

-- ─────────────────────────────────────────────────────────────
-- Bookmarks
-- ─────────────────────────────────────────────────────────────
CREATE TABLE note_bookmark (
    id           uuid PRIMARY KEY,
    citizen_id   uuid NOT NULL REFERENCES citizen(id) ON DELETE CASCADE,
    -- The Note object URI — either one of ours (federation_outbox_entry.activity_id
    -- namespace, resolvable to /actors/<h>/activities/<uuid>) or a remote one
    -- (federation_timeline_entry.object_uri). Not FK'd deliberately: bookmarks
    -- survive if we later purge remote timeline entries.
    object_uri   text NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (citizen_id, object_uri)
);
CREATE INDEX note_bookmark_citizen_created_idx
    ON note_bookmark (citizen_id, created_at DESC);

COMMENT ON TABLE note_bookmark IS
    '0.20.0-beta (fase 2A): per-citizen bookmark of any note by object_uri.';

-- ─────────────────────────────────────────────────────────────
-- Mutes
-- ─────────────────────────────────────────────────────────────
CREATE TABLE actor_mute (
    id                uuid PRIMARY KEY,
    citizen_id        uuid NOT NULL REFERENCES citizen(id) ON DELETE CASCADE,
    -- URL that identifies the target actor cross-instance. Local actors have
    -- an actor_url too (see federation.rs::actor_id).
    target_actor_url  text NOT NULL,
    -- If true, also drop notifications originating from this actor. Matches
    -- Mastodon's `notifications=true` toggle.
    hide_notifications boolean NOT NULL DEFAULT true,
    created_at        timestamptz NOT NULL DEFAULT now(),
    UNIQUE (citizen_id, target_actor_url)
);
CREATE INDEX actor_mute_citizen_idx
    ON actor_mute (citizen_id);
COMMENT ON TABLE actor_mute IS
    '0.20.0-beta: soft-hide of another actor from the caller''s feed + notifs.';

-- ─────────────────────────────────────────────────────────────
-- Blocks
-- ─────────────────────────────────────────────────────────────
CREATE TABLE actor_block (
    id                uuid PRIMARY KEY,
    citizen_id        uuid NOT NULL REFERENCES citizen(id) ON DELETE CASCADE,
    target_actor_url  text NOT NULL,
    created_at        timestamptz NOT NULL DEFAULT now(),
    UNIQUE (citizen_id, target_actor_url)
);
CREATE INDEX actor_block_citizen_idx
    ON actor_block (citizen_id);
COMMENT ON TABLE actor_block IS
    '0.20.0-beta: hard block — mute + refuse follow + drop inbound reactions.';

-- ─────────────────────────────────────────────────────────────
-- Content filters
-- ─────────────────────────────────────────────────────────────
CREATE TABLE content_filter (
    id           uuid PRIMARY KEY,
    citizen_id   uuid NOT NULL REFERENCES citizen(id) ON DELETE CASCADE,
    phrase       text NOT NULL CHECK (length(phrase) BETWEEN 1 AND 400),
    -- Contexts where the filter applies. Any subset of the Mastodon-defined
    -- {'home','notifications','public','thread','account'} — enforced only in
    -- the app layer to keep this schema forward-compatible.
    context      text[] NOT NULL DEFAULT ARRAY['home']::text[],
    -- NULL = permanent; a future timestamp = auto-expires and the app filters
    -- it out at read time.
    expires_at   timestamptz,
    created_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX content_filter_citizen_idx
    ON content_filter (citizen_id);
COMMENT ON TABLE content_filter IS
    '0.20.0-beta: substring content filter, optionally auto-expiring.';

-- ─────────────────────────────────────────────────────────────
-- Lists (Mastodon `Lists API`)
-- ─────────────────────────────────────────────────────────────
CREATE TABLE actor_list (
    id           uuid PRIMARY KEY,
    citizen_id   uuid NOT NULL REFERENCES citizen(id) ON DELETE CASCADE,
    title        text NOT NULL CHECK (length(title) BETWEEN 1 AND 100),
    -- Mirrors Mastodon's `replies_policy`: which replies from list members to
    -- include in the list timeline. 'followed' = show replies to accounts the
    -- viewer already follows; 'list' = only replies to list members; 'none' =
    -- exclude replies entirely.
    replies_policy text NOT NULL DEFAULT 'list'
                   CHECK (replies_policy IN ('followed','list','none')),
    created_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX actor_list_citizen_idx
    ON actor_list (citizen_id);

CREATE TABLE actor_list_member (
    list_id           uuid NOT NULL REFERENCES actor_list(id) ON DELETE CASCADE,
    target_actor_url  text NOT NULL,
    created_at        timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (list_id, target_actor_url)
);
CREATE INDEX actor_list_member_url_idx
    ON actor_list_member (target_actor_url);

COMMENT ON TABLE actor_list IS
    '0.20.0-beta: user-owned list of accounts (Mastodon Lists API).';
COMMENT ON TABLE actor_list_member IS
    '0.20.0-beta: junction table for list membership by target_actor_url.';

COMMIT;

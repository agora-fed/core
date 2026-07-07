-- 0405_note_hashtag_and_mention — Mastodon parity fase 1 (0.18.0).
-- Two junction tables that index #tags and @mentions extracted from every
-- Note (local + remote). Populated by the Rust extractor at publish and at
-- inbound receipt so the timelines and search endpoints can join by tag/
-- actor without re-parsing content_html on every read.
--
-- Design notes:
-- * `object_uri` (text) is the canonical join key across both local
--   (federation_outbox_entry.activity_id-derived object id) and remote
--   (federation_timeline_entry.object_uri) sides.
-- * Hashtags are stored NORMALIZED (lowercase, unicode-NFC, diacritics
--   stripped) so `#SaúdePública` and `#saudepublica` collide in the index.
--   The raw as-authored form is kept in `tag_original` for display.
-- * Mentions are stored as `mentioned_actor_url` (canonical) + a
--   `mentioned_handle` copy for cheap display without a lookup. Local
--   mentions resolve to a `citizen.handle`; remote mentions carry the full
--   `user@remote.tld` handle.

CREATE TABLE note_hashtag (
    id              uuid PRIMARY KEY,
    -- Which Note this tag belongs to. Text (not FK) because it may reference
    -- either the local outbox side or the remote timeline side.
    object_uri      text NOT NULL,
    -- Normalized form (lowercase, no accents, no leading '#'). Query key.
    tag_normalized  text NOT NULL,
    -- As it appears in the source text (leading '#' stripped). For display.
    tag_original    text NOT NULL,
    created_at      timestamptz NOT NULL,
    -- One row per (Note, tag). A repeated hashtag inside the same Note is
    -- collapsed at insert-time by the extractor.
    UNIQUE (object_uri, tag_normalized)
);

CREATE INDEX note_hashtag_normalized_idx
    ON note_hashtag (tag_normalized, created_at DESC);

CREATE TABLE note_mention (
    id                    uuid PRIMARY KEY,
    object_uri            text NOT NULL,
    -- Actor URL of the mentioned party. For local mentions this is
    -- `<base>/actors/<handle>`; for remote mentions the full remote URL.
    mentioned_actor_url   text NOT NULL,
    -- Denormalized display handle (e.g. `alice` for local, `alice@remote.tld`
    -- for remote). Kept so the feed can render mentions without a lookup.
    mentioned_handle      text NOT NULL,
    created_at            timestamptz NOT NULL,
    UNIQUE (object_uri, mentioned_actor_url)
);

-- Notification / mailbox query: "give me all Notes that mention this actor".
CREATE INDEX note_mention_actor_idx
    ON note_mention (mentioned_actor_url, created_at DESC);

COMMENT ON TABLE note_hashtag IS
    'ADR-0010 W3 (0.18.0): hashtag junction, normalized for case-insensitive lookup.';
COMMENT ON TABLE note_mention IS
    'ADR-0010 W3 (0.18.0): mention junction, target actor URL + display handle.';

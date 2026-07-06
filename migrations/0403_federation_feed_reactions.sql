-- 0403_federation_feed_reactions — federated timeline + Like/Announce reactions (ADR-0010 W2.6).
-- Range 0400-0409 is federation per migrations/REGISTRY.md.
--
-- Three tables:
--
-- 1. `federation_timeline_entry` — REMOTE Notes received via the inbox (`Create(Note)` from an
--    actor at least one local citizen follows). Stored ONCE per `object_uri`, regardless of how
--    many local citizens follow the author — each citizen's feed filters by their own
--    `federation_follow (direction='outbound')` rows at read time. Author metadata (handle,
--    display name, avatar) is denormalized from the signer's Actor document at receipt time so
--    the feed renders without any remote fetch.
--
-- 2. `federation_reaction` — LOCAL reactions (a citizen liked/boosted an object, local or
--    remote). `activity_id` is the id of the `Like`/`Announce` activity WE emitted, kept so a
--    toggle-off can deliver a well-formed `Undo` carrying the original inner activity id
--    (Mastodon matches Undo on it). UNIQUE (citizen, object, kind) makes the toggle idempotent.
--
-- 3. `federation_remote_reaction` — REMOTE reactions received via the inbox (`Like`/`Announce`
--    over one of OUR objects). One row per (remote actor, object, kind); a re-delivered Like is
--    a no-op via the UNIQUE. `Undo` deletes the row.

CREATE TABLE federation_timeline_entry (
    id                  uuid PRIMARY KEY,
    -- The Note object's `id` URI — the global dedupe key (one row per object across all
    -- followers). Treated as opaque text.
    object_uri          text NOT NULL UNIQUE,
    -- The author's Actor URL — the join key against `federation_follow.remote_actor_url`
    -- when assembling a citizen's feed.
    actor_url           text NOT NULL,
    -- Denormalized author identity captured from the signer's Actor doc at receipt time.
    -- `actor_handle` renders as `user@remote.tld` (no leading @) in the feed contract.
    actor_handle        text NOT NULL,
    actor_display_name  text,
    actor_avatar_url    text,
    -- The Note's `content` as delivered (already-sanitized server-side before insert; capped at
    -- 64 KiB in the handler — the column CHECK is the last-line guard).
    content_html        text NOT NULL CHECK (length(content_html) <= 65536),
    -- The remote `published` stamp (fallback: receipt time) — the feed sort key.
    published_at        timestamptz NOT NULL,
    received_at         timestamptz NOT NULL
);

-- Feed assembly: WHERE actor_url IN (my follows) ORDER BY published_at DESC.
CREATE INDEX federation_timeline_actor_published_idx
    ON federation_timeline_entry (actor_url, published_at DESC);
CREATE INDEX federation_timeline_published_idx
    ON federation_timeline_entry (published_at DESC);

CREATE TABLE federation_reaction (
    id           uuid PRIMARY KEY,
    -- Who reacted (always a LOCAL citizen — remote reactions live in the sibling table).
    citizen_id   uuid NOT NULL REFERENCES citizen(id),
    -- What they reacted to: a local outbox object OR a remote timeline object. Opaque text —
    -- no FK because the target may live in either table (or be remote-only).
    object_uri   text NOT NULL,
    kind         text NOT NULL CHECK (kind IN ('like', 'boost')),
    -- Id of the `Like`/`Announce` activity WE emitted (used to build the `Undo` on toggle-off).
    activity_id  text NOT NULL,
    created_at   timestamptz NOT NULL,
    -- The toggle key: one like + one boost per (citizen, object).
    UNIQUE (citizen_id, object_uri, kind)
);

-- Count aggregation per object (feed like_count/boost_count).
CREATE INDEX federation_reaction_object_kind_idx
    ON federation_reaction (object_uri, kind);

CREATE TABLE federation_remote_reaction (
    id                uuid PRIMARY KEY,
    -- The remote reactor's Actor URL (the inbox signer).
    remote_actor_url  text NOT NULL,
    -- One of OUR object URIs (validated against federation_outbox_entry at receipt).
    object_uri        text NOT NULL,
    kind              text NOT NULL CHECK (kind IN ('like', 'boost')),
    -- The remote `Like`/`Announce` activity id (matched by `Undo`, kept for ops).
    activity_id       text,
    created_at        timestamptz NOT NULL,
    -- Re-delivered reactions are no-ops; `Undo` deletes by (actor, object, kind).
    UNIQUE (remote_actor_url, object_uri, kind)
);

CREATE INDEX federation_remote_reaction_object_kind_idx
    ON federation_remote_reaction (object_uri, kind);

COMMENT ON TABLE federation_timeline_entry IS
    'ADR-0010 W2.6: remote Notes received via inbox, one row per object_uri (feed source).';
COMMENT ON TABLE federation_reaction IS
    'ADR-0010 W2.6: local citizen Like/Announce reactions (toggle), with our activity id for Undo.';
COMMENT ON TABLE federation_remote_reaction IS
    'ADR-0010 W2.6: remote Like/Announce over our objects, received via inbox.';

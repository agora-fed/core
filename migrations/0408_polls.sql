-- 0408_polls — Mastodon parity: polls on Notes (ActivityStreams `Question`).
--
-- Three tables:
--
-- 1. `note_poll` — one row per poll. Linked to a Note via `object_uri` (the
--    Note's AP object id). `multiple` toggles single-choice vs multi-select
--    ("oneOf" vs "anyOf" in AS vocab). `expires_at` is authoritative — once
--    passed, the API rejects votes and marks `closed_at`.
-- 2. `note_poll_option` — one row per option. `sort_order` preserves the
--    author's ordering; `vote_count` is denormalized from `note_poll_vote`
--    so the feed renders bars without a per-row aggregation.
-- 3. `note_poll_vote` — one row per (poll, actor). For multi-select we store
--    the chosen option ids in a `text[]` (uuid array serialization) to keep
--    the vote atomic (one row = one voter's full ballot). Idempotent via
--    UNIQUE (poll_id, actor_url).

CREATE TABLE note_poll (
    id            uuid PRIMARY KEY,
    -- The Note's AP object URI (matches federation_outbox_entry.object or
    -- federation_timeline_entry.object_uri). Text (not FK) for the same
    -- reasons as note_hashtag / note_media.
    object_uri    text NOT NULL UNIQUE,
    -- False = single-choice (AS `oneOf`), True = multi-select (AS `anyOf`).
    multiple      boolean NOT NULL DEFAULT false,
    expires_at    timestamptz NOT NULL,
    closed_at     timestamptz,
    created_at    timestamptz NOT NULL,
    CONSTRAINT note_poll_expires_after_create_chk
        CHECK (expires_at > created_at)
);

CREATE INDEX note_poll_expires_idx
    ON note_poll (expires_at)
    WHERE closed_at IS NULL;

CREATE TABLE note_poll_option (
    id            uuid PRIMARY KEY,
    poll_id       uuid NOT NULL REFERENCES note_poll(id) ON DELETE CASCADE,
    sort_order    integer NOT NULL,
    text          text NOT NULL CHECK (length(text) > 0 AND length(text) <= 200),
    -- Denormalized count; updated at vote time. `count_recompute` (future
    -- migration) can rebuild this from note_poll_vote if it ever drifts.
    vote_count    integer NOT NULL DEFAULT 0,
    UNIQUE (poll_id, sort_order)
);

CREATE INDEX note_poll_option_poll_idx
    ON note_poll_option (poll_id, sort_order);

CREATE TABLE note_poll_vote (
    id            uuid PRIMARY KEY,
    poll_id       uuid NOT NULL REFERENCES note_poll(id) ON DELETE CASCADE,
    -- Voter's Actor URL. For local citizens: `{public_origin}/actors/{handle}`.
    -- Remote voters carry their full remote URL. `text` (not FK) — no join to
    -- citizen so remote votes work uniformly.
    actor_url     text NOT NULL,
    -- The chosen option ids. For single-choice this is a 1-element array; for
    -- multi-select it may hold every option chosen. Kept as text[] of uuid so
    -- one vote row = one voter's whole ballot.
    option_ids    text[] NOT NULL,
    created_at    timestamptz NOT NULL,
    UNIQUE (poll_id, actor_url)
);

CREATE INDEX note_poll_vote_poll_idx
    ON note_poll_vote (poll_id, created_at DESC);

COMMENT ON TABLE note_poll IS
    'ADR-0010 W3 (0.18.0-rc1): AS Question wired to a Note; oneOf/anyOf via `multiple`.';
COMMENT ON TABLE note_poll_option IS
    'ADR-0010 W3 (0.18.0-rc1): ordered options of a poll with denormalized vote_count.';
COMMENT ON TABLE note_poll_vote IS
    'ADR-0010 W3 (0.18.0-rc1): one row per voter — the full ballot for multi-select polls.';

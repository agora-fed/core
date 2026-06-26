-- 0402_federation_notes_and_delivery — Notes posted by citizens + per-recipient delivery queue
-- (ADR-0010 W2.5). Range 0400-0409 is federation per migrations/REGISTRY.md.
--
-- Two tables:
--
-- 1. `federation_outbox_entry` — one row per Activity the citizen authored. Currently always a
--    `Create(Note)`, but the `kind` column leaves room for future Update/Delete/Announce
--    activities without a schema change. The activity payload is stored verbatim as JSONB so
--    the worker delivers EXACTLY the bytes that the Outbox `GET` returns — a re-render from
--    domain fields could drift from what followers cached.
--
-- 2. `federation_delivery` — one row per (outbox_entry, follower inbox). At-least-once delivery
--    with exponential backoff: `attempts` counts tries, `next_attempt_at` gates the worker's
--    next pull, `delivered_at` flips on the first 2xx and the row is then inert. `last_error`
--    is kept for ops introspection ("why didn't Mastodon X get my last note?").

CREATE TABLE federation_outbox_entry (
    id            uuid PRIMARY KEY,
    citizen_id    uuid NOT NULL REFERENCES citizen(id),
    -- AP activity id URL (publicly dereferenceable: `<actor>/activities/<uuid>`). Embedded in
    -- the payload but exposed separately so the Outbox endpoint can build OrderedCollection
    -- without parsing JSONB.
    activity_id   text NOT NULL UNIQUE,
    -- Activity type: 'Create' covers the only kind we emit today. Future: 'Announce', etc.
    kind          text NOT NULL CHECK (kind IN ('Create')),
    -- Visibility envelope. 'public' = addressed to `as:Public` + the actor's followers (the
    -- "post-to-the-world" Mastodon default). Other modes (unlisted/followers-only/direct) are
    -- reserved for later; today's UI only emits 'public'.
    visibility    text NOT NULL CHECK (visibility IN ('public', 'unlisted', 'followers', 'direct'))
                  DEFAULT 'public',
    -- The wire-ready Activity (the Create wrapping the Note). The Outbox endpoint serves this
    -- straight back; the delivery worker POSTs this verbatim to each follower's inbox.
    payload       jsonb NOT NULL,
    created_at    timestamptz NOT NULL
);

CREATE INDEX federation_outbox_entry_citizen_created_idx
    ON federation_outbox_entry (citizen_id, created_at DESC);

CREATE TABLE federation_delivery (
    id                uuid PRIMARY KEY,
    outbox_entry_id   uuid NOT NULL REFERENCES federation_outbox_entry(id) ON DELETE CASCADE,
    -- The recipient inbox URL (cached from `federation_follow.remote_inbox_url` at fanout time).
    -- Stored as text so a follower's inbox can move between fetches without breaking the row.
    recipient_inbox   text NOT NULL,
    attempts          integer NOT NULL DEFAULT 0,
    -- Set to NOW() on insert; bumped by the worker on each failure with exponential backoff.
    next_attempt_at   timestamptz NOT NULL,
    -- Stamped on the first 2xx; the row is then "done" and the worker stops pulling it.
    delivered_at      timestamptz,
    last_error        text,
    created_at        timestamptz NOT NULL,
    -- One delivery per (entry, inbox). A re-enqueue of the same fanout is a no-op via the
    -- ON CONFLICT path the service uses.
    UNIQUE (outbox_entry_id, recipient_inbox)
);

-- Drives the worker's poll: pending rows ready to ship, ordered FIFO inside the window.
-- Partial index so it stays tiny when most rows have been delivered.
CREATE INDEX federation_delivery_pending_idx
    ON federation_delivery (next_attempt_at, id)
    WHERE delivered_at IS NULL;

COMMENT ON TABLE federation_outbox_entry IS
    'ADR-0010 W2.5: citizen-authored ActivityPub activities (Create/Note today).';
COMMENT ON TABLE federation_delivery IS
    'ADR-0010 W2.5: per-recipient at-least-once delivery queue with exp backoff.';

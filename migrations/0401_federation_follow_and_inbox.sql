-- 0401_federation_follow_and_inbox — the social graph + inbox idempotency (ADR-0010 W2.2).
--
-- Two tables, both under the federation range (0400-0409 per REGISTRY.md):
--
-- 1. `federation_follow` — the social graph. Each row is ONE direction of one follow relation.
--    The opaque `activity_id` from the remote `Follow` activity is the natural idempotency key:
--    a duplicate Follow (Mastodon retries on a slow Accept) is a no-op via `ON CONFLICT`.
--
-- 2. `federation_inbox_seen` — strict idempotency log for inbound activity ids. Mastodon retries
--    inbound deliveries on any non-2xx for up to ~6 weeks, so the inbox handler MUST be safe
--    to re-execute on the same `id`. We insert into this table BEFORE acting; a duplicate insert
--    is the signal "already processed, return 202 fast".

CREATE TABLE federation_follow (
    -- Surrogate id — opaque, not the activity_id.
    id                  uuid PRIMARY KEY,
    -- Which of our public citizens is the OWNER of this follow relation. For an inbound follow
    -- ("someone follows our Esposa") this is Esposa's id. For an outbound follow (W2.4: "Esposa
    -- follows someone remote") this is also Esposa's id — `direction` says which side she's on.
    citizen_id          uuid NOT NULL REFERENCES citizen(id),
    -- 'inbound'  = someone remote follows our citizen (we are the followee)
    -- 'outbound' = our citizen follows someone remote (we are the follower) [W2.4]
    direction           text NOT NULL CHECK (direction IN ('inbound', 'outbound')),
    -- Remote ActivityPub Actor URL of the OTHER side (the follower for inbound; the followee for
    -- outbound). Treated as opaque text — no parsing, no normalization beyond what the wire gave.
    remote_actor_url    text NOT NULL,
    -- Remote actor's inbox URL (cached from the fetched Actor doc at follow time, used to
    -- deliver Accept now and future activities later). NULL until the first successful fetch.
    remote_inbox_url    text,
    -- Opaque activity id of the inbound `Follow` (for dedupe). NULL on outbound (we generate
    -- our own id and store that).
    follow_activity_id  text,
    -- Did we ACK with Accept? Set on successful outbound Accept delivery for inbound follows;
    -- always true for outbound follows once the remote sends Accept back [W2.4].
    accepted_at         timestamptz,
    created_at          timestamptz NOT NULL,
    -- One direction-scoped relation per (owner, remote_actor) — `ON CONFLICT (citizen_id,
    -- direction, remote_actor_url)` makes a duplicate Follow a no-op.
    UNIQUE (citizen_id, direction, remote_actor_url)
);

CREATE INDEX federation_follow_citizen_direction_idx
    ON federation_follow (citizen_id, direction, created_at DESC);

-- The OPAQUE inbox dedupe log. Every successful inbox POST writes one row here BEFORE acting.
-- A re-delivery hits the unique-violation and returns 202 immediately (no double-action).
CREATE TABLE federation_inbox_seen (
    -- The remote activity's `id` URI. Trusted as opaque; we never parse it for routing.
    activity_id  text PRIMARY KEY,
    -- Which of OUR citizens received this delivery (for ops queries / GC).
    citizen_id   uuid NOT NULL REFERENCES citizen(id),
    seen_at      timestamptz NOT NULL
);

CREATE INDEX federation_inbox_seen_citizen_idx
    ON federation_inbox_seen (citizen_id, seen_at DESC);

COMMENT ON TABLE federation_follow IS
    'ADR-0010 W2.2: bidirectional follow graph (inbound or outbound) keyed by remote actor URL.';
COMMENT ON TABLE federation_inbox_seen IS
    'ADR-0010 W2.2: idempotency log for ActivityPub inbox deliveries. Insert-before-act.';

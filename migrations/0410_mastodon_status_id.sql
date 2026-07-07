-- 0410_mastodon_status_id — id ↔ object_uri lookup for the Mastodon Client API.
--
-- Mastodon clients treat `Status.id` as an opaque string. We derive it from
-- the ActivityPub `object_uri` via SHA-256, but that hash is one-way, so a
-- follow-up call like `POST /api/v1/statuses/{id}/favourite` needs to
-- recover the URI. This table holds the mapping — populated on-demand the
-- first time we serialize a Status (from either the local outbox or the
-- remote timeline).
--
-- One row per Note the wire ever surfaced. Deleting the Note does NOT
-- cascade — a subsequent tombstone lookup still resolves. Rows are trimmed
-- by a future GC when the object_uri has been deleted_at for > 30d.

CREATE TABLE mastodon_status_id (
    -- The public id we hand to Mastodon clients. Deterministic (SHA-256 hex,
    -- first 22 base64url chars) so re-serialization returns the same id.
    id            text PRIMARY KEY,
    object_uri    text NOT NULL UNIQUE,
    created_at    timestamptz NOT NULL
);

CREATE INDEX mastodon_status_id_created_idx
    ON mastodon_status_id (created_at DESC);

COMMENT ON TABLE mastodon_status_id IS
    'ADR-0010 (0.19.0): id ↔ object_uri lookup for Mastodon Client API status endpoints.';

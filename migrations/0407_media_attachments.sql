-- 0407_media_attachments — Mastodon parity fase 3 (0.18.0-gamma).
-- Media attachments on Notes. Two tables:
--
-- 1. `media_attachment` — the file itself (one row per uploaded artifact).
--    Key columns: `object_key` (MinIO/S3 key inside our `dsoc-media` bucket),
--    `content_type`, `kind` (image/video/audio), optional dimensions and
--    `alt_text`. `actor_url` is the uploader's Actor URL — we can later
--    garbage-collect orphans (rows never linked to a note).
--
-- 2. `note_media` — junction linking a Note URI to one or more attachments
--    in order. Idempotent via UNIQUE (object_uri, media_id). Deletion of the
--    parent Note (soft or hard) does NOT cascade because notes are stored
--    as jsonb payloads with soft-delete; a nightly sweep can drop orphans.
--
-- Design notes:
-- * `object_key` (text) is the storage key inside our bucket (e.g.
--   `notes/2026/07/019f3aaa-....jpg`). The public URL is built by the API
--   as `{MEDIA_BASE_URL}/{object_key}` at serve time so we can migrate
--   buckets without a data rewrite.
-- * `alt_text` is capped at 1500 chars (Mastodon caps at 1500 too).
-- * `blurhash` is optional; we only compute it for images. Text.
-- * `sort_order` is a small integer starting at 0. Mastodon caps at 4
--   attachments per note; we mirror that at the API layer, not the schema.

CREATE TABLE media_attachment (
    id            uuid PRIMARY KEY,
    -- Uploader's Actor URL. Local uploads: `{public_origin}/actors/{handle}`.
    -- Remote uploads (federated Note with attachment[]): the remote actor URL.
    actor_url     text NOT NULL,
    -- image | video | audio (Mastodon uses `image`/`video`/`audio`/`gifv`;
    -- we keep the base three today and add `gifv` in a later migration).
    kind          text NOT NULL CHECK (kind IN ('image', 'video', 'audio')),
    -- Storage key inside our MinIO bucket. NULL for remote-only entries
    -- (where we just cache the remote URL in `remote_url`).
    object_key    text,
    -- For remote attachments we don't proxy: cache the source URL. When
    -- `object_key` is set the front should use our own /media path.
    remote_url    text,
    content_type  text NOT NULL,
    -- Accessibility caption, optional (max 1500 chars).
    alt_text      text,
    width         integer,
    height        integer,
    duration_ms   integer,
    -- Blurhash for image placeholders; NULL for non-images.
    blurhash      text,
    -- Byte length of the stored artifact (NULL for remote-only rows).
    size_bytes    bigint,
    created_at    timestamptz NOT NULL,
    CONSTRAINT media_attachment_alt_len_chk
        CHECK (alt_text IS NULL OR length(alt_text) <= 1500),
    -- Either local (object_key set) or remote (remote_url set) — not neither.
    CONSTRAINT media_attachment_source_chk
        CHECK (object_key IS NOT NULL OR remote_url IS NOT NULL)
);

CREATE INDEX media_attachment_actor_created_idx
    ON media_attachment (actor_url, created_at DESC);

CREATE TABLE note_media (
    id            uuid PRIMARY KEY,
    -- The Note object URI this attachment is bound to.
    object_uri    text NOT NULL,
    media_id      uuid NOT NULL REFERENCES media_attachment(id) ON DELETE CASCADE,
    sort_order    integer NOT NULL DEFAULT 0,
    created_at    timestamptz NOT NULL,
    UNIQUE (object_uri, media_id)
);

CREATE INDEX note_media_object_idx
    ON note_media (object_uri, sort_order);

COMMENT ON TABLE media_attachment IS
    'ADR-0010 W3 (0.18.0-gamma): media artifacts (image/video/audio) for Notes.';
COMMENT ON TABLE note_media IS
    'ADR-0010 W3 (0.18.0-gamma): junction linking Notes to their attachments (max 4 per note at API).';

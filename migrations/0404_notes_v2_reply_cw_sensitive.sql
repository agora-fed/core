-- 0404_notes_v2_reply_cw_sensitive — Mastodon parity fase 1 (0.18.0).
-- Adds threading, content warnings and sensitive-media flag to both the LOCAL
-- author side (federation_outbox_entry) and the REMOTE-received side
-- (federation_timeline_entry). Also adds soft edit/delete markers.
--
-- Design notes:
-- * `in_reply_to_uri` stores the parent Note's object URI (text) rather than a
--   foreign key: the parent may live in ANY of {this instance's outbox,
--   remote federation_timeline_entry, an entirely uncached remote instance},
--   so a FK would over-constrain. Threading traversal is done via URI joins.
-- * `spoiler_text` is Mastodon's CW header (Content Warning). It's a plain
--   text summary shown before the collapsed content. `sensitive=true` without
--   spoiler_text means "media flagged" (used later for image blur).
-- * `edited_at` NULL → never edited; NOT NULL → the last edit stamp. The
--   history table (note_revision) is deferred to 0.18.0 phase 2 — for now the
--   marker alone lets the front render an "edited" pill.
-- * `deleted_at` NULL → live; NOT NULL → tombstone (soft delete). Federated
--   `Delete(Note)` sets this on both tables; feed queries filter it out.

-- ─────────────────────────────────────────────────────────────
-- LOCAL author side (federation_outbox_entry)
-- ─────────────────────────────────────────────────────────────
ALTER TABLE federation_outbox_entry
    ADD COLUMN in_reply_to_uri text,
    ADD COLUMN sensitive       boolean NOT NULL DEFAULT false,
    ADD COLUMN spoiler_text    text,
    ADD COLUMN edited_at       timestamptz,
    ADD COLUMN deleted_at      timestamptz;

-- spoiler_text has a hard cap. Mastodon uses 500 chars — we mirror it.
ALTER TABLE federation_outbox_entry
    ADD CONSTRAINT federation_outbox_entry_spoiler_text_len_chk
    CHECK (spoiler_text IS NULL OR length(spoiler_text) <= 500);

-- Thread lookup: given a URI, find its direct children (replies to it).
CREATE INDEX federation_outbox_in_reply_to_idx
    ON federation_outbox_entry (in_reply_to_uri)
    WHERE in_reply_to_uri IS NOT NULL AND deleted_at IS NULL;

COMMENT ON COLUMN federation_outbox_entry.in_reply_to_uri IS
    'ADR-0010 W3 (0.18.0): parent Note object URI. NULL = top-level post.';
COMMENT ON COLUMN federation_outbox_entry.sensitive IS
    'ADR-0010 W3 (0.18.0): Mastodon-style sensitive flag (media/text).';
COMMENT ON COLUMN federation_outbox_entry.spoiler_text IS
    'ADR-0010 W3 (0.18.0): Content Warning header (max 500 chars).';
COMMENT ON COLUMN federation_outbox_entry.deleted_at IS
    'ADR-0010 W3 (0.18.0): soft-delete tombstone stamp; filter feeds by IS NULL.';

-- ─────────────────────────────────────────────────────────────
-- REMOTE-received side (federation_timeline_entry)
-- ─────────────────────────────────────────────────────────────
ALTER TABLE federation_timeline_entry
    ADD COLUMN in_reply_to_uri text,
    ADD COLUMN sensitive       boolean NOT NULL DEFAULT false,
    ADD COLUMN spoiler_text    text,
    ADD COLUMN edited_at       timestamptz,
    ADD COLUMN deleted_at      timestamptz;

-- Remote instances can drift on spoiler length — we cap defensively at 1024
-- rather than 500 (be permissive on inbound, strict on outbound). Truncation
-- happens in the handler; the CHECK is a last-line guard.
ALTER TABLE federation_timeline_entry
    ADD CONSTRAINT federation_timeline_entry_spoiler_text_len_chk
    CHECK (spoiler_text IS NULL OR length(spoiler_text) <= 1024);

CREATE INDEX federation_timeline_in_reply_to_idx
    ON federation_timeline_entry (in_reply_to_uri)
    WHERE in_reply_to_uri IS NOT NULL AND deleted_at IS NULL;

COMMENT ON COLUMN federation_timeline_entry.in_reply_to_uri IS
    'ADR-0010 W3 (0.18.0): parent Note object URI as received from the remote.';
COMMENT ON COLUMN federation_timeline_entry.deleted_at IS
    'ADR-0010 W3 (0.18.0): soft-delete stamp set by inbound Delete(Note).';

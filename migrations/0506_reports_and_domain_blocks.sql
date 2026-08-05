-- 0506_reports_and_domain_blocks.sql
--
-- Two pieces that complete the Mastodon-style post menu:
--
-- 1. `note_report` — reports submitted by citizens about publications
--    (ours OR the fediverse's). It becomes a work queue for human moderation.
--    The category follows Mastodon's vocabulary (spam / violation / other)
--    so it is easy to recycle in a future export.
-- 2. `domain_block` — a block at the whole-HOST level (e.g. blocking every
--    of `pravda.social.example`). We hide every note coming from that host
--    from the citizen's feed. This differs from `actor_block`, which is per account.

CREATE TABLE note_report (
    id                 uuid PRIMARY KEY,
    -- Who reports. No CASCADE because we want the historical record
    -- even if the account disappears (moderation audit).
    reporter_id        uuid NOT NULL REFERENCES citizen(id),
    -- object_uri of the reported note. Since remote notes have no FK, we leave
    -- solto igual a note_bookmark — sobrevive a purge do timeline remoto.
    object_uri         text NOT NULL,
    -- Author of the note (actor_url) — to ease the "how many reports has
    -- this account accumulated" question.
    author_actor_url   text NOT NULL,
    -- Fixed category of the report. A Mastodon-compatible vocabulary.
    category           text NOT NULL CHECK (category IN ('spam', 'violation', 'other')),
    -- Free text from the reporter (optional, up to 2000 chars).
    reason             text,
    created_at         timestamptz NOT NULL DEFAULT now(),
    -- Human moderation updates it. NULL = the queue.
    resolved_at        timestamptz,
    resolved_by        uuid REFERENCES citizen(id),
    -- The moderator's notes (never exposed to the reporter).
    resolution_notes   text,
    -- A citizen reports the same note only once.
    UNIQUE (reporter_id, object_uri)
);

CREATE INDEX note_report_pending_idx
    ON note_report (created_at DESC)
    WHERE resolved_at IS NULL;
CREATE INDEX note_report_author_idx
    ON note_report (author_actor_url);

COMMENT ON TABLE note_report IS
    '0.26.9 (mastodon-parity fase 2C): fila de denúncias submetidas pelo cidadão.';

ALTER TABLE note_report OWNER TO dsoc;

-- ─────────────────────────────────────────────────────────────
-- Whole-domain block
-- ─────────────────────────────────────────────────────────────
CREATE TABLE domain_block (
    id            uuid PRIMARY KEY,
    citizen_id    uuid NOT NULL REFERENCES citizen(id) ON DELETE CASCADE,
    -- Host normalized to lowercase (no scheme, no port). E.g. "pravda.example".
    domain        text NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    UNIQUE (citizen_id, domain)
);
CREATE INDEX domain_block_citizen_idx
    ON domain_block (citizen_id);

COMMENT ON TABLE domain_block IS
    '0.26.9: cidadão esconde do próprio feed qualquer nota vinda desse host.';

ALTER TABLE domain_block OWNER TO dsoc;

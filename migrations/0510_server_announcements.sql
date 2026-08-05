-- 0510_server_announcements.sql — announcements for the whole instance.
--
-- A yellow banner (or a variant) shown to any visitor — above the
-- header or as a persistent toast. Used for scheduled downtime,
-- rule changes, operational news. The admin creates/edits, the citizen only reads.
--
-- The Mastodon pattern:
--   * Markdown-lite text (sanitized before rendering).
--   * optional display window (starts_at .. ends_at). NULL = open.
--   * published_at: when it left draft. An unpublished one never appears.

CREATE TABLE server_announcement (
    id           uuid PRIMARY KEY,
    -- The announcement's text (Markdown-lite; the front end sanitizes).
    body         text NOT NULL,
    -- 'info' | 'warning' | 'critical'. UI muda a cor conforme.
    severity     text NOT NULL DEFAULT 'info' CHECK (severity IN ('info', 'warning', 'critical')),
    starts_at    timestamptz,
    ends_at      timestamptz,
    published_at timestamptz,
    created_at   timestamptz NOT NULL DEFAULT now(),
    created_by   uuid REFERENCES citizen(id)
);

CREATE INDEX server_announcement_published_idx
    ON server_announcement (published_at DESC)
    WHERE published_at IS NOT NULL;

COMMENT ON TABLE server_announcement IS
    '0.26.17: anúncios servidor-wide (banner pra downtime, avisos).';

ALTER TABLE server_announcement OWNER TO dsoc;

-- Dismissals — each citizen closes an announcement locally and it disappears for them.
CREATE TABLE server_announcement_dismissal (
    id                   uuid PRIMARY KEY,
    announcement_id      uuid NOT NULL REFERENCES server_announcement(id) ON DELETE CASCADE,
    citizen_id           uuid NOT NULL REFERENCES citizen(id) ON DELETE CASCADE,
    dismissed_at         timestamptz NOT NULL DEFAULT now(),
    UNIQUE (announcement_id, citizen_id)
);
COMMENT ON TABLE server_announcement_dismissal IS
    '0.26.17: cidadão fechou o banner, some pra ele.';
ALTER TABLE server_announcement_dismissal OWNER TO dsoc;

-- 0512_fediverso_completo.sql — custom emojis, hashtag moderation, auto-delete.
--
-- Three things that complete the "full Mastodon" package:
--   1. custom_emoji: shortcodes that become <img> at render time. PNG upload.
--   2. hashtag_moderation: admin banir hashtag (some do trending + feed
--      public feed) or promoting one so it always appears in trending.
--   3. citizen.auto_delete_notes_older_than_days: worker apaga notas
--      own posts older than N days.

CREATE TABLE custom_emoji (
    id           uuid PRIMARY KEY,
    -- Shortcode without `:` (e.g. 'party_dbr'). Case-sensitive, unique.
    shortcode    text NOT NULL UNIQUE
                 CHECK (shortcode ~ '^[A-Za-z0-9_-]+$'
                        AND char_length(shortcode) BETWEEN 2 AND 32),
    -- URL relativa /media/emoji/{uuid}.png. Front sempre monta como
    -- absolute with our own host.
    url          text NOT NULL,
    -- When false, it disappears from the picker + the render (history is kept).
    enabled      boolean NOT NULL DEFAULT true,
    created_at   timestamptz NOT NULL DEFAULT now(),
    created_by   uuid REFERENCES citizen(id)
);
COMMENT ON TABLE custom_emoji IS
    '0.26.19: emojis personalizados da instância. Renderiza como <img> por :shortcode:.';
ALTER TABLE custom_emoji OWNER TO dsoc;

-- ─────────────────────────────────────────────────────────────
-- Hashtag moderation
-- ─────────────────────────────────────────────────────────────
CREATE TABLE hashtag_moderation (
    -- Normalized tag (lowercase, no #).
    tag           text PRIMARY KEY CHECK (tag = lower(tag)),
    -- 'banned' hides it from trending and from the public feed.
    -- 'promoted' forces it into trending (even without volume).
    state         text NOT NULL CHECK (state IN ('banned', 'promoted')),
    reason        text,
    created_at    timestamptz NOT NULL DEFAULT now(),
    created_by    uuid REFERENCES citizen(id)
);
CREATE INDEX hashtag_moderation_state_idx ON hashtag_moderation (state);
COMMENT ON TABLE hashtag_moderation IS
    '0.26.19: banir hashtags do trending/feed público ou promovê-las.';
ALTER TABLE hashtag_moderation OWNER TO dsoc;

-- ─────────────────────────────────────────────────────────────
-- Automated deletion of posts
-- ─────────────────────────────────────────────────────────────
ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS auto_delete_notes_older_than_days integer
        CHECK (auto_delete_notes_older_than_days IS NULL
               OR auto_delete_notes_older_than_days >= 7);

COMMENT ON COLUMN citizen.auto_delete_notes_older_than_days IS
    '0.26.19: NULL = manter tudo. N = worker apaga notas próprias com idade > N dias.';

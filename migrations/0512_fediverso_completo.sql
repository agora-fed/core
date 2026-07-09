-- 0512_fediverso_completo.sql — emojis custom, moderação de hashtag, auto-delete.
--
-- Três coisas que fecham o pacote "Mastodon completo":
--   1. custom_emoji: shortcodes que viram <img> no render. Upload PNG.
--   2. hashtag_moderation: admin banir hashtag (some do trending + feed
--      público) ou promover pra sempre aparecer no trending.
--   3. citizen.auto_delete_notes_older_than_days: worker apaga notas
--      próprias com idade > N dias.

CREATE TABLE custom_emoji (
    id           uuid PRIMARY KEY,
    -- Shortcode sem `:` (ex.: 'party_dbr'). Case-sensitive, único.
    shortcode    text NOT NULL UNIQUE
                 CHECK (shortcode ~ '^[A-Za-z0-9_-]+$'
                        AND char_length(shortcode) BETWEEN 2 AND 32),
    -- URL relativa /media/emoji/{uuid}.png. Front sempre monta como
    -- absolute com o próprio host.
    url          text NOT NULL,
    -- Se falso, some do picker + do render (mantém histórico).
    enabled      boolean NOT NULL DEFAULT true,
    created_at   timestamptz NOT NULL DEFAULT now(),
    created_by   uuid REFERENCES citizen(id)
);
COMMENT ON TABLE custom_emoji IS
    '0.26.19: emojis personalizados da instância. Renderiza como <img> por :shortcode:.';
ALTER TABLE custom_emoji OWNER TO dsoc;

-- ─────────────────────────────────────────────────────────────
-- Moderação de hashtag
-- ─────────────────────────────────────────────────────────────
CREATE TABLE hashtag_moderation (
    -- Tag normalizada (lowercase, sem #).
    tag           text PRIMARY KEY CHECK (tag = lower(tag)),
    -- 'banned' esconde do trending e do feed público.
    -- 'promoted' força aparecer no trending (mesmo sem volume).
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
-- Exclusão automatizada de publicações
-- ─────────────────────────────────────────────────────────────
ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS auto_delete_notes_older_than_days integer
        CHECK (auto_delete_notes_older_than_days IS NULL
               OR auto_delete_notes_older_than_days >= 7);

COMMENT ON COLUMN citizen.auto_delete_notes_older_than_days IS
    '0.26.19: NULL = manter tudo. N = worker apaga notas próprias com idade > N dias.';

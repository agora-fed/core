-- 0513_terms_and_cw_presets.sql — editable Terms + CW presets.
--
-- 1. server_terms: a single row (fixed id), the Terms as Markdown-lite text.
--    If empty, /termos falls back to the hardcoded Astro content.
-- 2. cw_preset: list of terms that auto-suggest flagging a CW.

CREATE TABLE server_terms (
    id            integer PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    body          text NOT NULL,
    updated_at    timestamptz NOT NULL DEFAULT now(),
    updated_by    uuid REFERENCES citizen(id)
);
COMMENT ON TABLE server_terms IS
    '0.26.20: texto dos Termos editável pelo admin. Uma linha só (id=1).';
ALTER TABLE server_terms OWNER TO dsoc;

-- ─────────────────────────────────────────────────────────────
CREATE TABLE cw_preset (
    id            uuid PRIMARY KEY,
    -- Trigger: if the post text contains this substring (case-insensitive)
    -- the composer suggests flagging CW and optionally prefixes spoiler_text.
    phrase        text NOT NULL,
    -- Suggested label for spoiler_text. If NULL, it only flags sensitive.
    spoiler_text  text,
    created_at    timestamptz NOT NULL DEFAULT now(),
    created_by    uuid REFERENCES citizen(id),
    UNIQUE (phrase)
);
COMMENT ON TABLE cw_preset IS
    '0.26.20: predefinições de aviso — compose sugere CW quando o texto casa.';
ALTER TABLE cw_preset OWNER TO dsoc;

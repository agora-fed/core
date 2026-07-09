-- 0513_terms_and_cw_presets.sql — Termos editáveis + CW presets.
--
-- 1. server_terms: única linha (id fixo), texto Markdown-lite dos Termos.
--    Se estiver vazia, /termos cai no conteúdo hardcoded do Astro.
-- 2. cw_preset: lista de termos que auto-sugere marcar CW.

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
    -- Trigger: se o texto do post contém essa substring (case-insensitive)
    -- o compose sugere marcar CW e opcionalmente prefixa spoiler_text.
    phrase        text NOT NULL,
    -- Sugestão de rótulo pro spoiler_text. Se NULL, só marca sensível.
    spoiler_text  text,
    created_at    timestamptz NOT NULL DEFAULT now(),
    created_by    uuid REFERENCES citizen(id),
    UNIQUE (phrase)
);
COMMENT ON TABLE cw_preset IS
    '0.26.20: predefinições de aviso — compose sugere CW quando o texto casa.';
ALTER TABLE cw_preset OWNER TO dsoc;

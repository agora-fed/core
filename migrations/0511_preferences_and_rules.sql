-- 0511_preferences_and_rules.sql
--
-- Two things under "preferences and rules":
--
-- 1. Personal preferences on citizen: which events trigger e-mail and the
--    default compose visibility. Default: everything on, visibility
--    public.
-- 2. server_rule: the instance's ordered rule list, shown at signup
--    and on /sobre. Edited by the admin.

ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS email_prefs jsonb NOT NULL DEFAULT
        '{"mention":true,"reply":true,"favorite":true,"reblog":true,"follow":true,"admin_action":true}'::jsonb,
    ADD COLUMN IF NOT EXISTS default_visibility text NOT NULL DEFAULT 'public'
        CHECK (default_visibility IN ('public', 'unlisted', 'followers', 'direct')),
    ADD COLUMN IF NOT EXISTS default_sensitive boolean NOT NULL DEFAULT false;

COMMENT ON COLUMN citizen.email_prefs IS
    '0.26.18: bitmap por-evento pra desligar e-mail transacional. Vazio = tudo on.';
COMMENT ON COLUMN citizen.default_visibility IS
    '0.26.18: visibilidade padrão do compose. Mudança não afeta notas já publicadas.';

-- ─────────────────────────────────────────────────────────────
-- Instance rules
-- ─────────────────────────────────────────────────────────────
CREATE TABLE server_rule (
    id            uuid PRIMARY KEY,
    -- Display order (small positive integer). Duplicates allowed — resolved
    -- by (ordinal, created_at).
    ordinal       integer NOT NULL DEFAULT 0,
    text          text NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    created_by    uuid REFERENCES citizen(id)
);

CREATE INDEX server_rule_order_idx
    ON server_rule (ordinal, created_at);

COMMENT ON TABLE server_rule IS
    '0.26.18: regras da instância exibidas no cadastro e em /sobre.';

ALTER TABLE server_rule OWNER TO dsoc;

-- 0509_invitations.sql — convites de conta (invitation).
--
-- Um cidadão gera um token que outra pessoa usa no cadastro pra criar conta
-- na instância. Diferente do "mandate_invite" (que atribui um mandato a um
-- político — migration 0140s). Aqui é conta-nova de qualquer cidadão.
--
-- Padrão mastodon:
--   - Token URL-safe curto (~24 chars).
--   - Expiração opcional (default: 7 dias).
--   - Uso múltiplo opcional (`max_uses` default 1).
--   - Notes livres pra o convidante lembrar quem/porquê.
--   - Revogação por delete.

CREATE TABLE invitation (
    id                    uuid PRIMARY KEY,
    -- Cidadão que gerou o convite. Sem CASCADE — o registro histórico fica
    -- se o convidante sumir.
    invited_by_citizen_id uuid NOT NULL REFERENCES citizen(id),
    -- Token URL-safe. Case-sensitive; a UNIQUE cobre o lookup.
    token                 text NOT NULL UNIQUE,
    -- Convite direcionado a um e-mail específico. Se NULL, aceita qualquer
    -- endereço no cadastro. Se preenchido, o handler compara case-insensitive.
    target_email          text,
    -- Notas do convidante — não aparecem pro convidado.
    notes                 text,
    -- Quantas vezes ainda pode ser usado. Zero = esgotado.
    uses_left             integer NOT NULL DEFAULT 1 CHECK (uses_left >= 0),
    -- Total original — pra a UI mostrar "usado 2 de 5".
    max_uses              integer NOT NULL DEFAULT 1 CHECK (max_uses > 0),
    created_at            timestamptz NOT NULL DEFAULT now(),
    expires_at            timestamptz,
    -- Último uso — pra mostrar "usado por primeira vez em".
    first_used_at         timestamptz,
    last_used_at          timestamptz
);

CREATE INDEX invitation_by_citizen_idx
    ON invitation (invited_by_citizen_id, created_at DESC);
-- O UNIQUE em token já cria índice; para lookups o handler filtra uses_left
-- e expires_at em runtime.

COMMENT ON TABLE invitation IS
    '0.26.15: convite de conta — token URL-safe gerado por cidadão pra alguém criar conta.';

ALTER TABLE invitation OWNER TO dsoc;

-- Vincular signup a um convite: adiciona coluna opcional em citizen apontando
-- pro convite usado. Facilita "quem convidou quem" no admin.
ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS invited_via_invitation_id uuid REFERENCES invitation(id);

COMMENT ON COLUMN citizen.invited_via_invitation_id IS
    '0.26.15: se a conta foi criada via /convite?token=X, aponta pro registro.';

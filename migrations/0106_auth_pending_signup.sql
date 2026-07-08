-- Migration 0106 — verificação de e-mail antes da criação da conta.
--
-- Fluxo hoje: POST /auth/register cria citizen + credential + session
-- imediatamente. Fluxo novo: /auth/register grava um pending_signup +
-- envia link por e-mail; /auth/register/confirm redime o token e cria a
-- conta atomicamente. Nenhuma linha em `citizen` até a verificação passar
-- — CPF não fica "preso" por e-mails inválidos ou sock-puppets.
--
-- O token é HASHEADO em repouso (SHA-256, mesmo padrão de
-- auth_password_reset — migration 0103). O plaintext só existe no e-mail.
--
-- Também: muda o default de `citizen.is_public` para true. Cidadãos
-- verificados por e-mail já apareceriam nas buscas / no fediverso —
-- alinhamento com padrão Mastodon (opt-out em Configurações, não opt-in).
-- (Backfill de contas antigas fica a critério do operador.)

BEGIN;

CREATE TABLE auth_pending_signup (
    id             uuid PRIMARY KEY,
    org_id         uuid NOT NULL REFERENCES org(id),
    -- E-mail normalizado (trim + lowercase). Não é UNIQUE: pode haver duas
    -- pending pra mesmo e-mail — o vencedor é quem confirmar primeiro, e o
    -- perdedor recebe conflito no INSERT em auth_credential (que É unique
    -- por (org_id, email) via migration 0101).
    email          text NOT NULL,
    -- Argon2id hash da senha, produzido no request e reutilizado no confirm.
    password_hash  text NOT NULL,
    -- CPF só normalizado (11 dígitos, algorítmico já checado).
    cpf            text NOT NULL,
    -- 'cidadao' | 'politico'. Determina qual serviço materializa a conta
    -- no confirm (register vs register_politician).
    role           text NOT NULL
        CHECK (role IN ('cidadao','politico')),
    -- Só populado quando role='politico'. Validação já feita no request
    -- (email == mandate.public_email) — o confirm só re-materializa.
    mandate_id     uuid,
    -- SHA-256 do token URL-safe. Plaintext nunca persistido (mesmo padrão
    -- do password_reset).
    token_hash     bytea NOT NULL,
    -- TTL curto (ver AUTH_SIGNUP_VERIFY_TTL_SECS, default 24h).
    expires_at     timestamptz NOT NULL,
    -- Set em confirmação bem-sucedida. NULL = redimível.
    used_at        timestamptz,
    -- IP de origem, best-effort (audit).
    request_ip     text,
    created_at     timestamptz NOT NULL,

    -- Consistência role/mandate: politico ⇒ mandate_id NOT NULL.
    CHECK (role = 'cidadao' OR mandate_id IS NOT NULL)
);

-- Lookup por token_hash (path do confirm).
CREATE INDEX auth_pending_signup_token_hash_idx
    ON auth_pending_signup (token_hash);

-- Facilita "invalidar pending live pro mesmo e-mail" no request path
-- (mesma UX do password_reset: re-request substitui o anterior).
CREATE INDEX auth_pending_signup_email_live_idx
    ON auth_pending_signup (org_id, email)
    WHERE used_at IS NULL;

COMMENT ON TABLE auth_pending_signup IS
    '0.25.0-fediverso: signup pendente aguardando verificação de e-mail. '
    'Uma linha por request; token SHA-256 hasheado. Confirm materializa '
    'citizen+credential atomicamente.';
COMMENT ON COLUMN auth_pending_signup.token_hash IS
    'sha256(token); plaintext nunca persistido.';

-- Default de is_public: agora true. Novos cadastros aparecem em buscas /
-- webfinger sem exigir opt-in. Usuário desativa em Configurações → perfil.
-- (Contas já existentes ficam como estavam — sem backfill automático.)
ALTER TABLE citizen ALTER COLUMN is_public SET DEFAULT true;

-- Em prod, migrations rodam como `postgres` (via runbook) enquanto o gateway
-- conecta como `dsoc`. Sem OWNER explícito, novas tabelas ficam propriedade
-- do usuário rodando o script → 42501 "permission denied" em runtime.
-- Alinhado com o padrão do `citizen` (dsoc-owned). Idempotente.
ALTER TABLE auth_pending_signup OWNER TO dsoc;

COMMIT;

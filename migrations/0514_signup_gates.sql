-- 0514_signup_gates.sql — 3 tabelas de gate no cadastro/login + coluna
-- de aprovação em citizen.
--
-- 1. email_domain_block: e-mail de cadastro com domínio nessa tabela é
--    recusado.
-- 2. ip_rule: pool de allow (só esses IPs podem cadastrar) OU deny
--    (esses IPs não podem). state='allow'|'deny'.
-- 3. citizen.pending_review: quando true (setado por config global admin
--    no futuro; hoje default false), a conta existe mas não pode logar
--    até um admin aprovar.

CREATE TABLE email_domain_block (
    id           uuid PRIMARY KEY,
    -- Host normalizado lowercase, ex.: 'mailinator.com'.
    domain       text NOT NULL UNIQUE,
    reason       text,
    created_at   timestamptz NOT NULL DEFAULT now(),
    created_by   uuid REFERENCES citizen(id)
);
COMMENT ON TABLE email_domain_block IS
    '0.26.21: e-mails com esses domínios são recusados no cadastro.';
ALTER TABLE email_domain_block OWNER TO dsoc;

-- ─────────────────────────────────────────────────────────────
CREATE TABLE ip_rule (
    id           uuid PRIMARY KEY,
    -- CIDR ou IP único (armazenado como texto pra simplificar; validado
    -- em runtime). Ex.: '192.168.1.0/24' ou '203.0.113.5'.
    cidr         text NOT NULL,
    -- 'signup' bloqueia só cadastro; 'login' bloqueia login; 'all' os dois.
    scope        text NOT NULL CHECK (scope IN ('signup', 'login', 'all')),
    state        text NOT NULL CHECK (state IN ('allow', 'deny')),
    reason       text,
    created_at   timestamptz NOT NULL DEFAULT now(),
    created_by   uuid REFERENCES citizen(id),
    UNIQUE (cidr, scope)
);
CREATE INDEX ip_rule_state_idx ON ip_rule (state, scope);
COMMENT ON TABLE ip_rule IS
    '0.26.21: allow/deny de IP no cadastro/login. Empty allowlist = todos.';
ALTER TABLE ip_rule OWNER TO dsoc;

-- ─────────────────────────────────────────────────────────────
ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS pending_review boolean NOT NULL DEFAULT false,
    ADD COLUMN IF NOT EXISTS approved_at timestamptz,
    ADD COLUMN IF NOT EXISTS approved_by uuid REFERENCES citizen(id);
COMMENT ON COLUMN citizen.pending_review IS
    '0.26.21: quando true, conta criada mas sem poder logar. Precisa admin aprovar.';

-- Coluna global no server_terms pra saber se a instância exige revisão.
-- Alternativa: singleton em server_settings, mas 0.26.21 não faz isso.
-- Por enquanto: env var GATEWAY_SIGNUP_REQUIRES_REVIEW=true força pending.

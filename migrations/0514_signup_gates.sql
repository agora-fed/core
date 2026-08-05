-- 0514_signup_gates.sql — 3 tabelas de gate no cadastro/login + coluna
-- of approval on citizen.
--
-- 1. email_domain_block: a signup e-mail whose domain is in this table is
--    recusado.
-- 2. ip_rule: an allow pool (only these IPs may sign up) OR a deny one
--    (these IPs may not). state='allow'|'deny'.
-- 3. citizen.pending_review: when true (set by a global admin config
--    in future; today it defaults to false), the account exists but cannot log in
--    until an admin approves it.

CREATE TABLE email_domain_block (
    id           uuid PRIMARY KEY,
    -- Host normalized to lowercase, e.g. 'mailinator.com'.
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
    -- A CIDR or a single IP (stored as text for simplicity; validated
    -- em runtime). Ex.: '192.168.1.0/24' ou '203.0.113.5'.
    cidr         text NOT NULL,
    -- 'signup' blocks signup only; 'login' blocks login; 'all' blocks both.
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

-- A global column on server_terms to know whether the instance requires review.
-- Alternative: a singleton in server_settings, but 0.26.21 does not do that.
-- For now: the env var GATEWAY_SIGNUP_REQUIRES_REVIEW=true forces pending.

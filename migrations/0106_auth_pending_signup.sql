-- Migration 0106 — e-mail verification before the account is created.
--
-- Today's flow: POST /auth/register creates citizen + credential + session
-- immediately. The new flow: /auth/register stores a pending_signup and
-- sends a link by e-mail; /auth/register/confirm redeems the token and creates the
-- account atomically. No row in `citizen` until verification passes
-- — the identity document never gets "stuck" behind invalid e-mails or sock puppets.
--
-- The token is HASHED at rest (SHA-256, the same pattern as
-- auth_password_reset — migration 0103). The plaintext exists only in the e-mail.
--
-- Also: changes the default of `citizen.is_public` to true. Citizens
-- verified by e-mail would already appear in searches / on the fediverse —
-- aligned with the Mastodon standard (opt-out in Settings, not opt-in).
-- (Backfilling old accounts is left to the operator.)

BEGIN;

CREATE TABLE auth_pending_signup (
    id             uuid PRIMARY KEY,
    org_id         uuid NOT NULL REFERENCES org(id),
    -- Normalized e-mail (trim + lowercase). Not UNIQUE: there may be two
    -- pendings for the same e-mail — the winner is whoever confirms first, and the
    -- loser gets a conflict on the INSERT into auth_credential (which IS unique
-- unique per (org_id, email) via migration 0101).
    email          text NOT NULL,
    -- Argon2id password hash, produced on the request and reused on the confirm.
    password_hash  text NOT NULL,
    -- The identity document, only normalized (11 digits, algorithmically checked already).
    cpf            text NOT NULL,
    -- 'cidadao' | 'politico'. Determines which service materializes the account
    -- no confirm (register vs register_politician).
    role           text NOT NULL
        CHECK (role IN ('cidadao','politico')),
    -- Populated only when role='politico'. Validation already happened at request time
    -- (email == mandate.public_email) — confirm merely re-materializes.
    mandate_id     uuid,
    -- SHA-256 of the URL-safe token. The plaintext is never persisted (the same pattern
    -- do password_reset).
    token_hash     bytea NOT NULL,
    -- TTL curto (ver AUTH_SIGNUP_VERIFY_TTL_SECS, default 24h).
    expires_at     timestamptz NOT NULL,
    -- Set on a successful confirmation. NULL = redeemable.
    used_at        timestamptz,
    -- Origin IP, best-effort (audit).
    request_ip     text,
    created_at     timestamptz NOT NULL,

    -- role/mandate consistency: politico ⇒ mandate_id NOT NULL.
    CHECK (role = 'cidadao' OR mandate_id IS NOT NULL)
);

-- Lookup by token_hash (the confirm path).
CREATE INDEX auth_pending_signup_token_hash_idx
    ON auth_pending_signup (token_hash);

-- Eases "invalidate the live pending for the same e-mail" on the request path
-- (same UX as password_reset: a re-request replaces the previous one).
CREATE INDEX auth_pending_signup_email_live_idx
    ON auth_pending_signup (org_id, email)
    WHERE used_at IS NULL;

COMMENT ON TABLE auth_pending_signup IS
    '0.25.0-fediverso: signup pendente aguardando verificação de e-mail. '
    'Uma linha por request; token SHA-256 hasheado. Confirm materializa '
    'citizen+credential atomicamente.';
COMMENT ON COLUMN auth_pending_signup.token_hash IS
    'sha256(token); plaintext nunca persistido.';

-- Default for is_public: now true. New signups appear in searches /
-- webfinger without requiring an opt-in. The user disables it in Settings → profile.
-- (Existing accounts stay as they were — no automatic backfill.)
ALTER TABLE citizen ALTER COLUMN is_public SET DEFAULT true;

-- In prod, migrations run as `postgres` (via the runbook) while the gateway
-- connects as `dsoc`. Without an explicit OWNER, new tables end up owned
-- by the user running the script → a 42501 "permission denied" at runtime.
-- Aligned with the `citizen` pattern (dsoc-owned). Idempotent.
ALTER TABLE auth_pending_signup OWNER TO dsoc;

COMMIT;

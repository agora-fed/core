-- 0103_auth_password_reset — password-reset tokens (ADR-0010, W1). The token is a credential:
-- it is HASHED inside Postgres on insert (pgcrypto digest, sha256) so the plaintext never lives
-- in a column — same pattern as `mandate_invitation` (migration 0200). Lookup compares hashes.
--
-- Single-use, time-bound: a token may be confirmed at most once (the `used_at` write is gated by
-- a UNIQUE-via-NULL trick: see the partial index below).

CREATE TABLE auth_password_reset (
    id           uuid PRIMARY KEY,
    citizen_id   uuid NOT NULL REFERENCES citizen(id),
    -- SHA-256 of the URL-safe random token. Plaintext is never persisted.
    token_hash   bytea NOT NULL,
    -- Lifetime: a reset link expires; the service rejects after `expires_at`.
    expires_at   timestamptz NOT NULL,
    -- Set on successful redemption. NULL = redeemable. Once set, the row is dead.
    used_at      timestamptz,
    -- Best-effort source IP (text — works for both v4 and v6, no decoding needed). For audit.
    request_ip   text,
    created_at   timestamptz NOT NULL
);

-- Lookup by token hash. Postgres has `bytea` btree, fast equality.
CREATE INDEX auth_password_reset_token_hash_idx
    ON auth_password_reset (token_hash);

-- One LIVE reset at a time per citizen (partial unique). Re-requesting a reset before the prior
-- one expires invalidates the prior (the request service deletes any redeemable rows first).
CREATE INDEX auth_password_reset_citizen_live_idx
    ON auth_password_reset (citizen_id)
    WHERE used_at IS NULL;

COMMENT ON COLUMN auth_password_reset.token_hash IS
    'sha256(token); plaintext token never stored. Mirrors mandate_invitation pattern.';

-- 0104_mandate_invite — admin-driven invite to a parlamentar whose public_email on file is
-- wrong / institutional / missing (so the F1.4 self-registration auto-proof
-- `email == mandate.public_email` cannot match). An admin dispatches an invite to any e-mail;
-- clicking the link lets the parlamentar create the account + sign the `directory` binding.
--
-- Complements the existing `mandate_invitation` table (migration 0200), which onboards a
-- mandate as a whole (transition mandate.onboarded_at). This table is scoped to the *person*
-- taking control of the mandate on the platform: the accepted row records who invited whom,
-- which citizen was materialised on acceptance, and the resulting identity binding.
--
-- SECURITY: only a SHA-256 HASH of the token is stored (BYTEA), matching the
-- `auth_password_reset` pattern in migration 0103. The plaintext token exists only in the
-- outgoing e-mail and in the accept-URL — never in a database column, never in a log.
--
-- Range 0100-0109 belongs to the auth crate per migrations/REGISTRY.md (the service that owns
-- this flow lives in dsoc-auth; FKs only touch core identity tables, as the registry allows).

CREATE TABLE mandate_invite (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Tenant the invite lives in. FK on core `org` (mandate is already org-scoped, but a top-
    -- level org_id here lets the invite be listed for a partido/plataforma admin without a JOIN).
    org_id       uuid NOT NULL REFERENCES org(id),
    -- The mandate whose seat this invite targets.
    mandate_id   uuid NOT NULL REFERENCES mandate(id),
    -- Where the invite was sent. Compared case-insensitive on acceptance; kept as originally
    -- typed so the audit trail preserves the operator's intent.
    email        text NOT NULL,
    -- SHA-256 of the plaintext URL-safe token. NEVER the plaintext (SECURITY.md).
    token_hash   bytea NOT NULL,
    -- The admin/inviter (a citizen with, in a future migration, admin role over the partido
    -- or plataforma). Today: any authenticated citizen — the handler gates it with a TODO.
    invited_by   uuid NOT NULL REFERENCES citizen(id),
    -- Time bound. Beyond this the accept endpoint returns "expired" without touching state.
    expires_at   timestamptz NOT NULL,
    -- When the invite was redeemed. NULL = still redeemable (subject to expires_at + revoked_at).
    accepted_at  timestamptz,
    -- Optional revocation stamp (inviter or admin can invalidate). NULL = not revoked.
    revoked_at   timestamptz,
    created_at   timestamptz NOT NULL DEFAULT now()
);

-- Lookup by token hash on accept. `bytea` btree fast-equality.
CREATE INDEX mandate_invite_token_hash_idx
    ON mandate_invite (token_hash);

-- Pending-invites-for-a-mandate. Partial so the index only carries live rows.
CREATE INDEX mandate_invite_mandate_pending_idx
    ON mandate_invite (mandate_id)
    WHERE accepted_at IS NULL AND revoked_at IS NULL;

COMMENT ON TABLE mandate_invite IS
    'dsoc-mandates: admin-driven invites to a parlamentar. Only the token hash is stored.';
COMMENT ON COLUMN mandate_invite.token_hash IS
    'sha256(token); plaintext token never stored. Mirrors auth_password_reset (0103).';
COMMENT ON COLUMN mandate_invite.email IS
    'Target e-mail; compared case-insensitive on accept. May differ from mandate.public_email.';

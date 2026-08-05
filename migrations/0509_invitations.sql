-- 0509_invitations.sql — account invitations (invitation).
--
-- A citizen mints a token another person uses at signup to create an account
-- on the instance. Different from the "mandate_invite" (which assigns a mandate to an
-- official — migrations 0140s). This one is a new account for any citizen.
--
-- The Mastodon pattern:
--   - Token URL-safe curto (~24 chars).
--   - Optional expiry (default: 7 days).
--   - Optional multiple use (`max_uses` default 1).
--   - Free-form notes so the inviter remembers who/why.
--   - Revocation by delete.

CREATE TABLE invitation (
    id                    uuid PRIMARY KEY,
    -- The citizen who minted the invitation. No CASCADE — the historical record stays
    -- if the inviter disappears.
    invited_by_citizen_id uuid NOT NULL REFERENCES citizen(id),
    -- Token URL-safe. Case-sensitive; a UNIQUE cobre o lookup.
    token                 text NOT NULL UNIQUE,
    -- An invitation directed at a specific e-mail. When NULL it accepts any
    -- address at signup. When filled, the handler compares case-insensitively.
    target_email          text,
    -- The inviter's notes — never shown to the invitee.
    notes                 text,
    -- How many times it can still be used. Zero = exhausted.
    uses_left             integer NOT NULL DEFAULT 1 CHECK (uses_left >= 0),
    -- The original total — so the UI can show "used 2 of 5".
    max_uses              integer NOT NULL DEFAULT 1 CHECK (max_uses > 0),
    created_at            timestamptz NOT NULL DEFAULT now(),
    expires_at            timestamptz,
    -- Last use — to show "first used on".
    first_used_at         timestamptz,
    last_used_at          timestamptz
);

CREATE INDEX invitation_by_citizen_idx
    ON invitation (invited_by_citizen_id, created_at DESC);
-- The UNIQUE on token already creates an index; for lookups the handler filters uses_left
-- e expires_at em runtime.

COMMENT ON TABLE invitation IS
    '0.26.15: convite de conta — token URL-safe gerado por cidadão pra alguém criar conta.';

ALTER TABLE invitation OWNER TO dsoc;

-- Linking a signup to an invitation: adds an optional column on citizen pointing
-- for the used invitation. It eases "who invited whom" in the admin.
ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS invited_via_invitation_id uuid REFERENCES invitation(id);

COMMENT ON COLUMN citizen.invited_via_invitation_id IS
    '0.26.15: se a conta foi criada via /convite?token=X, aponta pro registro.';

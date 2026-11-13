-- 0693_initiative_public_signing — owned by `dsoc-initiatives` (public petitions surface).
--
-- Lets a person sign an initiative without an account. The person gives a name and an
-- e-mail address. The signature counts only after the person confirms the address.
-- On confirmation the gateway creates (or finds) the `citizen` row, so every counted
-- signature still references a real citizen (initiative_signature.citizen_id stays NOT NULL).
--
-- Two verification modes exist (see docs/petitions.md):
--   * local — this server sends the e-mail. `token_hash` holds SHA-256 of the link token.
--   * sso   — the identity provider sends a code. `sso_user_id` holds the provider user.
--
-- The cleartext e-mail in `initiative_pending_signature` is transient: the row is deleted
-- on confirmation and when it expires. No IPv4 anywhere (PLAN.md principle 4).

-- ---------------------------------------------------------------------------
-- initiative: who wrote it, and whether it still collects signatures.
--   open   — collects signatures, listed.
--   closed — the author or an admin stopped it; still readable.
--   hidden — an admin removed it from public view (moderation).
-- ---------------------------------------------------------------------------
ALTER TABLE initiative
    ADD COLUMN author_id uuid REFERENCES citizen(id),
    ADD COLUMN status    text NOT NULL DEFAULT 'open'
        CHECK (status IN ('open', 'closed', 'hidden')),
    ADD COLUMN closed_at timestamptz,
    -- true when an admin set the current status (moderation); the author cannot undo it.
    ADD COLUMN moderated boolean NOT NULL DEFAULT false;

CREATE INDEX initiative_org_status_idx ON initiative (org_id, status, created_at DESC);

-- ---------------------------------------------------------------------------
-- initiative_signature: what the signer agreed to show in public.
-- `public_name` is NULL unless the signer asked to show the name.
-- ---------------------------------------------------------------------------
ALTER TABLE initiative_signature
    ADD COLUMN public_name text CHECK (public_name IS NULL OR char_length(public_name) <= 80),
    ADD COLUMN city        text CHECK (city IS NULL OR char_length(city) <= 80);

-- ---------------------------------------------------------------------------
-- initiative_pending_signature: a signature that waits for e-mail confirmation.
-- ---------------------------------------------------------------------------
CREATE TABLE initiative_pending_signature (
    id            uuid PRIMARY KEY,
    initiative_id uuid NOT NULL REFERENCES initiative(id) ON DELETE CASCADE,
    -- Lower-cased address. Transient: the row goes away on confirmation or expiry.
    email         text NOT NULL CHECK (char_length(email) <= 254),
    name          text NOT NULL CHECK (char_length(name) BETWEEN 2 AND 80),
    city          text CHECK (city IS NULL OR char_length(city) <= 80),
    show_name     boolean NOT NULL DEFAULT false,
    -- local mode: SHA-256 of the single-use link token. NULL in sso mode.
    token_hash    bytea UNIQUE,
    -- sso mode: the provider's user id that received the code.
    sso_user_id   text,
    -- true when this server created that provider user (so it may remove it on expiry).
    sso_created   boolean NOT NULL DEFAULT false,
    created_at    timestamptz NOT NULL,
    -- Last time a confirmation e-mail went out. Drives the resend cooldown and the
    -- hourly cap, which must hold across restarts and across gateway processes.
    last_sent_at  timestamptz NOT NULL,
    expires_at    timestamptz NOT NULL,
    CHECK (token_hash IS NOT NULL OR sso_user_id IS NOT NULL)
);
-- One open request per address per initiative; a repeat request refreshes it.
CREATE UNIQUE INDEX initiative_pending_signature_email_uq
    ON initiative_pending_signature (initiative_id, email);
CREATE INDEX initiative_pending_signature_expiry_idx
    ON initiative_pending_signature (expires_at);
CREATE INDEX initiative_pending_signature_sent_idx
    ON initiative_pending_signature (last_sent_at);

-- ---------------------------------------------------------------------------
-- altcha_spent: proof-of-work challenges already used (replay guard).
-- ---------------------------------------------------------------------------
CREATE TABLE altcha_spent (
    -- The lower-case hex challenge: one canonical value per puzzle.
    challenge  text PRIMARY KEY,
    expires_at timestamptz NOT NULL
);
CREATE INDEX altcha_spent_expiry_idx ON altcha_spent (expires_at);

COMMENT ON TABLE initiative_pending_signature IS
    'Public petitions: signature waiting for e-mail confirmation. Transient PII, deleted on confirm/expiry.';
COMMENT ON TABLE altcha_spent IS
    'Public petitions: used ALTCHA proof-of-work challenges, kept until they expire.';

-- 0102_citizen_profile — extend the `citizen` table with the bare profile fields a real social
-- platform requires (display name, bio, user-chosen handle, avatar/cover storage keys, and a
-- public/private toggle). Per ADR-0010, citizens are PRIVATE by default; the federation surface
-- (per ADR-0005) materializes a Person Actor only when `is_public = true`.
--
-- This extension lives in the auth range (0100-0109) because identity + profile travel together
-- in this codebase (precedent: 0101_auth_credentials already extended `citizen` for credential
-- auth). The CPF is never derivable from any of these fields — `handle` is user-chosen, opaque,
-- and validated against the same character class Mastodon uses so federation handles like
-- `@ana@democracia.social.br` work without surprise.

ALTER TABLE citizen
    -- Friendly display name (shown in the header, in proposals, in comments). Optional: an
    -- anonymous citizen still has an opaque id-derived handle visible.
    ADD COLUMN display_name text,
    -- Short self-description. Hard cap is enforced at the API layer too; the DB cap is a
    -- last-line guard against an obvious payload-size attack.
    ADD COLUMN bio text,
    -- The user-chosen identifier surfaced in URLs and federation (acct:handle@host). Mastodon
    -- compatibility: ASCII letters, digits, underscore, dot, hyphen. Unique per org (multi-tenant
    -- aware) so two orgs can each have a `@ana`.
    ADD COLUMN handle text,
    -- Opaque object-storage keys for avatar/cover images. The bytes themselves live in MinIO
    -- (see crate `dsoc-storage`); the DB carries only the key, so a swap to another S3-compat
    -- backend is one config change. NULL = use the default rendered SVG (initials).
    ADD COLUMN avatar_object_key text,
    ADD COLUMN cover_object_key text,
    -- Privacy gate. FALSE = profile is local-only; federation (ADR-0005) NEVER materializes an
    -- Actor for this citizen, Webfinger returns 404 for the handle, and the directory/search
    -- excludes the citizen. The consequence-loop endpoints continue to function regardless —
    -- privacy hides the *face*, not the participation.
    ADD COLUMN is_public boolean NOT NULL DEFAULT false,
    -- Bumped on every profile mutation; lets the federation crate detect when to refresh the
    -- Actor object in remote inboxes (the AP `updated` timestamp).
    ADD COLUMN profile_updated_at timestamptz;

-- Handle constraints. The format is intentionally strict: it goes into URLs and into the
-- federation `acct:` URI, where every character that needs escaping is a future support ticket.
ALTER TABLE citizen
    ADD CONSTRAINT citizen_handle_format CHECK (
        handle IS NULL
        OR (handle ~ '^[A-Za-z0-9_.-]{3,32}$' AND handle NOT LIKE '%..%')
    ),
    -- Unique per org; nulls do not collide (Postgres default). Letting many citizens have no
    -- handle is fine — they show up as the opaque public_handle (u-<hex>) until they pick one.
    ADD CONSTRAINT citizen_org_handle_unique UNIQUE (org_id, handle);

-- Bio length cap (1000 chars). Generous for Brazilian Portuguese; UI advertises 500.
ALTER TABLE citizen
    ADD CONSTRAINT citizen_bio_length CHECK (
        bio IS NULL OR length(bio) <= 1000
    );

-- Display-name length cap (80 chars). Matches the federation `name` field's de-facto ceiling.
ALTER TABLE citizen
    ADD CONSTRAINT citizen_display_name_length CHECK (
        display_name IS NULL OR (length(display_name) BETWEEN 1 AND 80)
    );

-- Public-profile lookup: only needed when listing / searching public citizens. Partial so it
-- stays tiny on a "everyone-private" tenant.
CREATE INDEX citizen_public_idx ON citizen (org_id, handle) WHERE is_public = true;

COMMENT ON COLUMN citizen.is_public IS
    'ADR-0010: false = private (local only); true = materialize ActivityPub Person Actor.';
COMMENT ON COLUMN citizen.handle IS
    'User-chosen federation handle (Mastodon-compatible: [A-Za-z0-9_.-]{3,32}). NULL = use opaque public_handle.';

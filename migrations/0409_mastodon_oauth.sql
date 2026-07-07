-- 0409_mastodon_oauth — OAuth2 tables backing the Mastodon Client API (0.19.0).
--
-- Three tables cover the whole flow:
--
-- 1. `oauth_application` — one row per registered client (Ivory, Elk, Ice
--    Cubes, Tusky, custom). Client secret stored as SHA-256 hash so a DB
--    leak doesn't give attackers the secret. `redirect_uris` is an array
--    (Mastodon allows multiple), `scopes` is space-separated Mastodon-style
--    ("read write follow push").
-- 2. `oauth_authorization_code` — short-lived (10min TTL) codes issued from
--    /oauth/authorize, exchanged for an access token via /oauth/token
--    grant_type=authorization_code. `used_at` stamps first use so a stolen
--    code cannot be replayed.
-- 3. `oauth_access_token` — the actual bearer token. Stored as SHA-256 hash
--    for the same reason as client_secret. Long-lived by default (30 days,
--    matching the cookie session TTL); `revoked_at` supports explicit sign
--    out. Linked to `citizen` so the middleware can promote a valid bearer
--    into the same CallerId shape the cookie path produces.

CREATE TABLE oauth_application (
    id                 uuid PRIMARY KEY,
    -- Public client id shown to the app owner and echoed on every OAuth call.
    client_id          text NOT NULL UNIQUE,
    -- SHA-256 hash (hex) of the client_secret. Compare via constant-time cmp.
    client_secret_hash text NOT NULL,
    name               text NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
    -- Redirect URIs the client declared; multiple allowed (Mastodon parity).
    -- The exact URI in the flow MUST match one of these entries.
    redirect_uris      text[] NOT NULL,
    -- Space-separated Mastodon scopes: "read", "write", "follow", "push".
    scopes             text NOT NULL DEFAULT 'read',
    website            text,
    created_at         timestamptz NOT NULL
);

CREATE INDEX oauth_application_created_idx
    ON oauth_application (created_at DESC);

CREATE TABLE oauth_authorization_code (
    id             uuid PRIMARY KEY,
    application_id uuid NOT NULL REFERENCES oauth_application(id) ON DELETE CASCADE,
    citizen_id     uuid NOT NULL REFERENCES citizen(id),
    -- SHA-256 hash of the code. Compared via constant-time cmp in the handler.
    code_hash      text NOT NULL UNIQUE,
    redirect_uri   text NOT NULL,
    scopes         text NOT NULL,
    expires_at     timestamptz NOT NULL,
    used_at        timestamptz,
    created_at     timestamptz NOT NULL
);

-- Sweep index: /oauth/token deletes expired codes as it looks them up.
CREATE INDEX oauth_authorization_code_expires_idx
    ON oauth_authorization_code (expires_at)
    WHERE used_at IS NULL;

CREATE TABLE oauth_access_token (
    id             uuid PRIMARY KEY,
    application_id uuid NOT NULL REFERENCES oauth_application(id) ON DELETE CASCADE,
    -- Nullable because client_credentials grant issues a token that is NOT
    -- tied to a specific citizen (used for /api/v1/instance-shaped reads).
    citizen_id     uuid REFERENCES citizen(id),
    -- SHA-256 hash. The token itself is only ever returned once (at /oauth/token).
    token_hash     text NOT NULL UNIQUE,
    scopes         text NOT NULL,
    -- Long-lived by default (30 days, matches AUTH_SESSION_TTL_SECS in prod).
    expires_at     timestamptz NOT NULL,
    revoked_at     timestamptz,
    created_at     timestamptz NOT NULL
);

-- Fast lookup for the Authorization: Bearer <token> middleware.
CREATE INDEX oauth_access_token_hash_idx
    ON oauth_access_token (token_hash)
    WHERE revoked_at IS NULL;

CREATE INDEX oauth_access_token_citizen_idx
    ON oauth_access_token (citizen_id, created_at DESC)
    WHERE revoked_at IS NULL AND citizen_id IS NOT NULL;

COMMENT ON TABLE oauth_application IS
    'ADR-0010 (0.19.0): Mastodon Client API OAuth2 apps — client_id + hashed secret.';
COMMENT ON TABLE oauth_authorization_code IS
    'ADR-0010 (0.19.0): short-lived (10min TTL) codes exchanged at /oauth/token.';
COMMENT ON TABLE oauth_access_token IS
    'ADR-0010 (0.19.0): bearer tokens for the Mastodon Client API (hashed).';

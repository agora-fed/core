-- 0679_respond_link.sql — expiring, single-use, revocable response links (AGORA #12).
--
-- The reply-to-respond token was `hmac(secret, sla_id)`: deterministic, with no
-- temporal component. It never expired, could be replayed forever, and was not
-- individually revocable — only rotating the global secret invalidated anything, and
-- that invalidated EVERY link at once. Since `POST /respond` is unauthenticated and
-- writes a mandate's public official response, possession of a stale URL was standing
-- authority to speak in an official's name.
--
-- This table gives each link its own identity, so it can expire, be spent, and be
-- revoked on its own.
--
-- Only the SHA-256 of the token is stored. A database reader must not be able to mint
-- a working link, which is exactly what keeping the plaintext would allow — the same
-- rule the signup and password-reset tokens already follow.
--
-- `sla_id` is a soft-ref: consequence_sla belongs to another crate (REGISTRY rule).
--
-- Idempotent: rerun-safe.

BEGIN;

CREATE TABLE IF NOT EXISTS respond_link (
    id          uuid PRIMARY KEY,
    sla_id      uuid NOT NULL,
    -- SHA-256 of the token. Never the token itself.
    token_hash  bytea NOT NULL,
    expires_at  timestamptz NOT NULL,
    -- Set when the link is SPENT (a response was recorded). Single-use.
    used_at     timestamptz,
    -- Set to kill one link without touching the others.
    revoked_at  timestamptz,
    -- Failed presentations, so a guessing loop against one link is bounded.
    attempts    integer NOT NULL DEFAULT 0,
    created_at  timestamptz NOT NULL DEFAULT now()
);

-- The verification path: find the live link for an SLA.
CREATE INDEX IF NOT EXISTS respond_link_sla_idx
    ON respond_link (sla_id, expires_at DESC);

COMMENT ON TABLE respond_link IS
    '0679 (#12): expiring, single-use, revocable response links; only the token hash is stored.';

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'dsoc') THEN
        ALTER TABLE respond_link OWNER TO dsoc;
    END IF;
END $$;

COMMIT;

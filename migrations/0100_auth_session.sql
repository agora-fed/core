-- 0100_auth_session — owned by `dsoc-auth` (Tier 1). Sovereign identity & verification.
-- Explicit, auditable SQL (PLAN.md principle 3). Cross-crate FKs target ONLY core identity
-- tables (org, citizen) per ARCHITECTURE.md §3 / migrations/REGISTRY.md.

-- ---------------------------------------------------------------------------
-- auth_session — a validated-token exchange result: a server-issued session bound
-- to a sovereign OIDC subject. Carries the ActivityPub-readiness seam (ADR-0005).
-- ---------------------------------------------------------------------------
CREATE TABLE auth_session (
    id              uuid PRIMARY KEY,
    org_id          uuid NOT NULL REFERENCES org(id),
    citizen_id      uuid NOT NULL REFERENCES citizen(id),
    -- Sovereign identity subject (Zitadel OIDC `sub`); no foreign IdP.
    oidc_subject    text NOT NULL,
    issued_at       timestamptz NOT NULL,
    expires_at      timestamptz NOT NULL,
    -- ActivityPub-readiness seam (ADR-0005): a stable, public actor handle. Populated now
    -- so a citizen can later become an AP `Actor` without a schema retrofit.
    public_handle   text NOT NULL,
    -- ActivityPub-readiness seam (ADR-0005): reserved slot for a future HTTP-Signatures
    -- actor keypair (public key PEM). Storage reserved; NOT populated in the MVP.
    actor_public_key text,
    created_at      timestamptz NOT NULL
);
-- Keyset-friendly index: newest sessions per citizen first.
CREATE INDEX auth_session_citizen_idx ON auth_session (citizen_id, issued_at DESC, id DESC);

-- ---------------------------------------------------------------------------
-- auth_verification_level — append-only audit trail of every verification-level change.
-- A politically-targeted platform must be able to prove who was trusted, when, and why.
-- ---------------------------------------------------------------------------
CREATE TABLE auth_verification_level (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES org(id),
    citizen_id  uuid NOT NULL REFERENCES citizen(id),
    old_level   text NOT NULL
                CHECK (old_level IN ('anonymous','email','directory','strong')),
    new_level   text NOT NULL
                CHECK (new_level IN ('anonymous','email','directory','strong')),
    changed_at  timestamptz NOT NULL
);
-- Keyset pagination support: most recent change per citizen first.
CREATE INDEX auth_verification_level_citizen_idx
    ON auth_verification_level (citizen_id, changed_at DESC, id DESC);

-- ---------------------------------------------------------------------------
-- Citizen-facing session view (ADR-0005): exposes the stable public handle and session
-- window without leaking the OIDC subject. This is the seam future ActivityPub actor
-- resolution reads from.
-- ---------------------------------------------------------------------------
CREATE VIEW auth_session_public AS
SELECT id, org_id, citizen_id, public_handle, issued_at, expires_at
FROM auth_session;

COMMENT ON TABLE auth_session IS 'dsoc-auth: server-issued sessions; AP-readiness seam per ADR-0005.';
COMMENT ON TABLE auth_verification_level IS 'dsoc-auth: append-only verification-level audit trail.';

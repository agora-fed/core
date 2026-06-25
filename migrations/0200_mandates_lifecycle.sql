-- 0200_mandates_lifecycle — owned by `dsoc-mandates` (Tier 2, migration range 0200).
-- The registry binding real power to the consequence loop (Decidim failure #6): it drives
-- mandatory onboarding of public officials/candidates via their public email.
--
-- The base identity table `mandate` already exists in 0001_baseline (org-owned: office,
-- display_name, public_email, is_candidate, onboarded_at). This migration owns the LIFECYCLE
-- around it — it does NOT recreate it. Explicit, auditable SQL (PLAN.md principle 3).
-- Cross-crate FKs target ONLY core identity tables (`org`, `citizen`, `mandate`) per
-- migrations/REGISTRY.md. No IPv4 anywhere (PLAN.md principle 4).

-- ---------------------------------------------------------------------------
-- ActivityPub-readiness seam (ADR-0005): a reserved slot for a future HTTP-Signatures
-- actor keypair (public key PEM) on the mandate Actor. Storage reserved; NOT populated in
-- the MVP and AP is NOT implemented here. The mandate's stable public handle is derived
-- deterministically from its immutable id (see `dsoc_mandates::domain::public_handle`), so
-- it needs no stored column and never drifts. Additive, nullable: backward-compatible.
ALTER TABLE mandate ADD COLUMN IF NOT EXISTS actor_public_key text;

-- ---------------------------------------------------------------------------
-- mandate_office — a term-bound office record for a mandate (e.g. a 2025–2028 vereador
-- term in a given district). A mandate (one stable Actor identity) accrues offices as the
-- "voter → candidate → official" role progresses (ADR-0005).
-- ---------------------------------------------------------------------------
CREATE TABLE mandate_office (
    id          uuid PRIMARY KEY,
    mandate_id  uuid NOT NULL REFERENCES mandate(id),
    -- Office held or sought (e.g. 'vereador', 'prefeito', 'deputado_federal').
    office      text NOT NULL,
    -- Electoral district / municipality, when applicable.
    district    text,
    -- Term window (inclusive start, exclusive-ish end); NULL while unknown/ongoing.
    term_start  date,
    term_end    date,
    created_at  timestamptz NOT NULL
);
-- Keyset pagination support: list a single mandate's offices ordered by id.
CREATE INDEX mandate_office_mandate_idx ON mandate_office (mandate_id, id);

-- ---------------------------------------------------------------------------
-- mandate_invitation — a mandatory-onboarding invite sent to a mandate's public email.
-- SECURITY: only a HASH of the invite token is stored (`token_hash`), never the plaintext —
-- the token is a credential. Acceptance re-hashes the presented token and matches by hash.
-- ---------------------------------------------------------------------------
CREATE TABLE mandate_invitation (
    id           uuid PRIMARY KEY,
    mandate_id   uuid NOT NULL REFERENCES mandate(id),
    -- The public email the invite was addressed to (snapshot of mandate.public_email).
    public_email text NOT NULL,
    -- SHA-256 hex digest of the invite token. The plaintext token is NEVER persisted.
    token_hash   text NOT NULL,
    sent_at      timestamptz NOT NULL,
    -- Set once when the invite is accepted (onboarding). NULL while pending.
    accepted_at  timestamptz,
    created_at   timestamptz NOT NULL
);
-- Keyset pagination support: newest invitations per mandate first.
CREATE INDEX mandate_invitation_mandate_idx
    ON mandate_invitation (mandate_id, sent_at DESC, id DESC);
-- A token hash is globally unique: one invitation per issued token (lookup on acceptance).
CREATE UNIQUE INDEX mandate_invitation_token_hash_idx ON mandate_invitation (token_hash);

-- ---------------------------------------------------------------------------
-- mandate_identity_binding — append-only evidence binding a mandate to an identity-assurance
-- level (email-only → directory → TSE-backed). A politically-targeted platform must be able
-- to prove who was trusted as an official, at what level, when, and on what evidence.
-- ---------------------------------------------------------------------------
CREATE TABLE mandate_identity_binding (
    id                 uuid PRIMARY KEY,
    mandate_id         uuid NOT NULL REFERENCES mandate(id),
    -- Mirrors `dsoc_core::VerificationLevel`; kept in sync by this CHECK.
    verification_level text NOT NULL
                       CHECK (verification_level IN ('anonymous', 'email', 'directory', 'strong')),
    -- Free-form reference to the evidence (e.g. 'invitation:<id>', a TSE record id).
    evidence_ref       text,
    verified_at        timestamptz NOT NULL,
    created_at         timestamptz NOT NULL
);
-- Keyset pagination support: most recent binding per mandate first.
CREATE INDEX mandate_identity_binding_mandate_idx
    ON mandate_identity_binding (mandate_id, verified_at DESC, id DESC);

COMMENT ON TABLE mandate_office IS 'dsoc-mandates: term-bound office records for a mandate.';
COMMENT ON TABLE mandate_invitation IS 'dsoc-mandates: onboarding invites; only a token HASH is stored.';
COMMENT ON TABLE mandate_identity_binding IS 'dsoc-mandates: append-only identity-assurance evidence.';
COMMENT ON COLUMN mandate.actor_public_key IS 'AP-readiness seam (ADR-0005): reserved actor keypair; not populated in MVP.';

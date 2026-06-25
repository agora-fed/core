-- 0001_baseline — Tier-0 schema. Owned by `dsoc-db` / `dsoc-core`.
-- Explicit SQL, auditable (PLAN.md principle 3). PostgreSQL 16+ with pgvector.

CREATE EXTENSION IF NOT EXISTS vector;      -- pgvector, for consensus embeddings
CREATE EXTENSION IF NOT EXISTS pgcrypto;    -- gen_random_uuid fallback / hashing

-- ---------------------------------------------------------------------------
-- Identity tables (core-owned). Cross-crate FKs may reference ONLY these.
-- ---------------------------------------------------------------------------
CREATE TABLE org (
    id          uuid PRIMARY KEY,
    slug        text NOT NULL UNIQUE,
    name        text NOT NULL,
    created_at  timestamptz NOT NULL
);

CREATE TABLE citizen (
    id                  uuid PRIMARY KEY,
    org_id              uuid NOT NULL REFERENCES org(id),
    -- Sovereign identity subject (Zitadel OIDC `sub`); no foreign IdP.
    oidc_subject        text NOT NULL,
    verification_level  text NOT NULL DEFAULT 'anonymous'
                        CHECK (verification_level IN ('anonymous','email','directory','strong')),
    created_at          timestamptz NOT NULL,
    UNIQUE (org_id, oidc_subject)
);

CREATE TABLE mandate (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES org(id),
    -- Office held or sought (e.g. 'vereador', 'prefeito', 'deputado_federal', 'presidente').
    office      text NOT NULL,
    -- Public official/candidate name and PUBLIC contact email (mandatory onboarding).
    display_name    text NOT NULL,
    public_email    text NOT NULL,
    is_candidate    boolean NOT NULL DEFAULT false,
    onboarded_at    timestamptz,
    created_at      timestamptz NOT NULL
);

-- ---------------------------------------------------------------------------
-- Durable event log (owned by `dsoc-events`). The cross-crate contract on disk.
-- Append-only by convention; a hashed audit chain is added in Phase 3.
-- ---------------------------------------------------------------------------
CREATE TABLE events_log (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES org(id),
    topic       text NOT NULL,
    event_type  text NOT NULL,
    payload     jsonb NOT NULL,
    occurred_at timestamptz NOT NULL,
    -- Set when a subscriber has durably handled this delivery.
    processed_at timestamptz
);
CREATE INDEX events_log_topic_idx ON events_log (topic, occurred_at);
CREATE INDEX events_log_unprocessed_idx ON events_log (occurred_at) WHERE processed_at IS NULL;

-- ---------------------------------------------------------------------------
-- Migration provenance marker (in addition to sqlx's _sqlx_migrations table).
-- ---------------------------------------------------------------------------
COMMENT ON TABLE events_log IS 'Cross-crate event contract; see crates/core/src/events.rs';

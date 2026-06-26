-- 0230_initiatives_core — owned by `dsoc-initiatives` (Tier 2, migration range 0230).
-- Citizen initiatives: signature-threshold-driven proposals that escalate when supported
-- (CRATE.md). When an initiative gathers enough signatures it crosses its threshold and is
-- marked `reached_at` exactly once (idempotent escalation).
--
-- Explicit, auditable SQL (PLAN.md principle 3): no ORM, no SELECT *, keyset pagination on
-- lists. Cross-crate FKs target ONLY core identity tables (`org`, `citizen`) per
-- migrations/REGISTRY.md — `initiative_signature` references `citizen` and the same-migration
-- `initiative` table. No IPv4 anywhere (PLAN.md principle 4).

-- ---------------------------------------------------------------------------
-- initiative — a citizen-authored proposal that escalates once it gathers `threshold`
-- signatures. `signature_count` is a denormalized running tally maintained in the same
-- transaction as each signature insert (always equals the count of signature rows).
-- `reached_at` is the one-shot escalation marker: set exactly once, when the count first
-- reaches the threshold, and never cleared (idempotent via a `reached_at IS NULL` guard).
-- ---------------------------------------------------------------------------
CREATE TABLE initiative (
    id              uuid PRIMARY KEY,
    org_id          uuid NOT NULL REFERENCES org(id),
    -- Human title and full body of the initiative.
    title           text NOT NULL,
    body            text NOT NULL,
    -- Signatures required to escalate (>= 1, enforced in the domain and by this CHECK).
    threshold       integer NOT NULL CHECK (threshold >= 1),
    -- Running tally of signatures; maintained atomically with each signature insert.
    signature_count bigint NOT NULL DEFAULT 0 CHECK (signature_count >= 0),
    -- Set exactly once when the count first reaches the threshold; never cleared.
    reached_at      timestamptz,
    created_at      timestamptz NOT NULL
);
-- Keyset pagination support: list a single org's initiatives ordered by id.
CREATE INDEX initiative_org_idx ON initiative (org_id, id);

-- ---------------------------------------------------------------------------
-- initiative_signature — one citizen's signature on one initiative. The UNIQUE constraint
-- makes signing naturally idempotent: a citizen signs an initiative at most once, so a repeat
-- sign is a no-op that does not double-count.
-- ---------------------------------------------------------------------------
CREATE TABLE initiative_signature (
    id            uuid PRIMARY KEY,
    initiative_id uuid NOT NULL REFERENCES initiative(id),
    citizen_id    uuid NOT NULL REFERENCES citizen(id),
    created_at    timestamptz NOT NULL,
    UNIQUE (initiative_id, citizen_id)
);
-- Keyset pagination support: list a single initiative's signatures ordered by id.
CREATE INDEX initiative_signature_initiative_idx
    ON initiative_signature (initiative_id, id);

COMMENT ON TABLE initiative IS
    'dsoc-initiatives: citizen initiatives escalating at a signature threshold.';
COMMENT ON TABLE initiative_signature IS
    'dsoc-initiatives: one signature per (initiative, citizen); idempotent via UNIQUE.';
COMMENT ON COLUMN initiative.reached_at IS
    'One-shot escalation marker: set once when signature_count first reaches threshold.';

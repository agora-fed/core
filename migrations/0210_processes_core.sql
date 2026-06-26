-- 0210_processes_core — owned by `dsoc-processes` (Tier 2, migration range 0210).
-- Participatory processes: time-bound civic spaces grouping components toward a goal
-- (the Decidim "Participatory Process" concept, re-architected — credited in CHANGELOG).
--
-- Explicit, auditable SQL (PLAN.md principle 3): no ORM, no hidden callbacks. Cross-crate
-- FKs target ONLY core identity tables (`org`, `citizen`, `mandate`) per migrations/REGISTRY.md;
-- `process_phase` points at this crate's own `process` table (same migration). No IPv4 anywhere
-- (PLAN.md principle 4). Timestamps are `timestamptz`, written from the injected clock.

-- ---------------------------------------------------------------------------
-- process — a time-bound participation space. `status` drives its lifecycle:
--   draft  → being prepared, not yet open to participation;
--   active → open; components mounted into it accept participation;
--   closed → concluded; read-only.
-- The `(org_id, slug)` pair is the human-facing public handle and is unique per tenant.
-- ---------------------------------------------------------------------------
CREATE TABLE process (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES org(id),
    -- Stable, URL-safe handle, unique within the organization.
    slug        text NOT NULL,
    -- Human title (Portuguese civic content).
    title       text NOT NULL,
    -- Lifecycle state; mirrored by `dsoc_processes::domain::ProcessStatus`.
    status      text NOT NULL DEFAULT 'draft'
                CHECK (status IN ('draft', 'active', 'closed')),
    -- Optional participation window; NULL while unscheduled.
    starts_at   timestamptz,
    ends_at     timestamptz,
    created_at  timestamptz NOT NULL,
    UNIQUE (org_id, slug)
);
-- Keyset pagination support: list a single org's processes ordered by id.
CREATE INDEX process_org_idx ON process (org_id, id);

-- ---------------------------------------------------------------------------
-- process_phase — an ordered stage of a process (e.g. "Propostas", "Debate",
-- "Votação"). Exactly the phases of one process advance in `position` order; the
-- single `active` phase is the one currently open. `(process_id, position)` is unique
-- so the ordering is unambiguous and advancement is deterministic.
-- ---------------------------------------------------------------------------
CREATE TABLE process_phase (
    id          uuid PRIMARY KEY,
    process_id  uuid NOT NULL REFERENCES process(id) ON DELETE CASCADE,
    name        text NOT NULL,
    -- Zero-based ordinal within the process; unique so advancement is total-ordered.
    position    integer NOT NULL,
    -- Whether this phase is the currently open one. At most one true per process by convention.
    active      boolean NOT NULL DEFAULT false,
    created_at  timestamptz NOT NULL,
    UNIQUE (process_id, position)
);
-- Keyset pagination support: list a single process's phases ordered by position.
CREATE INDEX process_phase_process_idx ON process_phase (process_id, position);

COMMENT ON TABLE process IS 'dsoc-processes: time-bound participation spaces grouping components toward a goal.';
COMMENT ON TABLE process_phase IS 'dsoc-processes: ordered, advanceable stages of a process.';

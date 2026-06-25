-- 0390_scorecard_projection — owned by `dsoc-scorecard` (range 0390, see migrations/REGISTRY.md).
-- The permanent, public accountability artifact: a PURE PROJECTION from the consequence/mandate
-- event stream (Decidim failure #7 — Decidim never produced a per-politician public record). This
-- crate owns no business decisions; it rebuilds a politician's answered/ignored record and response
-- latency from an event-sourced ledger. Explicit, auditable SQL (PLAN.md principle 3): no ORM,
-- compile-time-checked by sqlx. PostgreSQL 16+.
--
-- Cross-crate foreign keys point ONLY at the core identity table `mandate` (ARCHITECTURE.md
-- section 3 / migrations/REGISTRY.md). SLA ids are stored as bare uuids: the consequence engine's
-- tables are owned by another crate, so we never FK across that boundary.

-- ---------------------------------------------------------------------------
-- scorecard — one public record per mandate. `answered`/`ignored` are denormalised
-- counters, but they are ALWAYS reconstructable from `scorecard_entry` (the ledger):
-- the service recomputes them from the ledger inside the same transaction as each
-- append, so the counters can never drift from the underlying entries.
-- ---------------------------------------------------------------------------
CREATE TABLE scorecard (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES org(id),
    -- Cross-crate FK to the core identity table `mandate`. UNIQUE: one scorecard per politician.
    mandate_id  uuid NOT NULL UNIQUE REFERENCES mandate(id),
    answered    bigint NOT NULL DEFAULT 0 CHECK (answered >= 0),
    ignored     bigint NOT NULL DEFAULT 0 CHECK (ignored >= 0),
    created_at  timestamptz NOT NULL,
    updated_at  timestamptz NOT NULL
);

-- Keyset pagination of an org's scorecards, newest-first by (created_at, id).
CREATE INDEX scorecard_org_idx ON scorecard (org_id, created_at DESC, id DESC);

-- ---------------------------------------------------------------------------
-- scorecard_entry — the event-sourced ledger. EXACTLY one row per consequence SLA
-- outcome. `sla_id` is UNIQUE so replaying the same `consequence.sla.expired` /
-- `consequence.official.responded` event (delivery is at-least-once — ADR-0006)
-- is idempotent: the duplicate insert is dropped and the counters never double-count.
-- A responded entry records its latency in `response_hours`; an ignored entry has none.
-- ---------------------------------------------------------------------------
CREATE TABLE scorecard_entry (
    id             uuid PRIMARY KEY,
    scorecard_id   uuid NOT NULL REFERENCES scorecard(id),
    -- Dedupe key for at-least-once delivery: one ledger row per originating SLA.
    sla_id         uuid NOT NULL UNIQUE,
    outcome        text NOT NULL CHECK (outcome IN ('answered', 'ignored')),
    response_hours double precision,
    occurred_at    timestamptz NOT NULL,
    created_at     timestamptz NOT NULL,
    -- An answered outcome must carry a non-negative latency; an ignored outcome must not.
    CONSTRAINT scorecard_entry_latency_consistency CHECK (
        (outcome = 'ignored'  AND response_hours IS NULL)
        OR (outcome = 'answered' AND response_hours IS NOT NULL AND response_hours >= 0)
    )
);

-- Recount the counters and pull answered latencies for the median, scoped to a scorecard.
CREATE INDEX scorecard_entry_scorecard_idx ON scorecard_entry (scorecard_id, outcome);

-- ---------------------------------------------------------------------------
-- scorecard_promise — public promises a politician makes and whether they delivered.
-- The other half of the accountability artifact ("promises vs delivery"). Recorded by
-- an authorized (directory-verified) official; `delivered` flips once, false -> true.
-- ---------------------------------------------------------------------------
CREATE TABLE scorecard_promise (
    id            uuid PRIMARY KEY,
    scorecard_id  uuid NOT NULL REFERENCES scorecard(id),
    text          text NOT NULL CHECK (length(text) > 0),
    made_at       timestamptz NOT NULL,
    delivered     boolean NOT NULL DEFAULT false,
    delivered_at  timestamptz,
    created_at    timestamptz NOT NULL,
    -- A promise is delivered iff it has a delivery timestamp.
    CONSTRAINT scorecard_promise_delivery_consistency CHECK (
        (delivered = false AND delivered_at IS NULL)
        OR (delivered = true AND delivered_at IS NOT NULL)
    )
);

-- Keyset pagination of a scorecard's promises, newest-first by (made_at, id).
CREATE INDEX scorecard_promise_scorecard_idx ON scorecard_promise (scorecard_id, made_at DESC, id DESC);

COMMENT ON TABLE scorecard IS 'Public per-politician accountability record; projection owned by dsoc-scorecard.';
COMMENT ON TABLE scorecard_entry IS 'Event-sourced ledger of SLA outcomes; counters are reconstructable from these rows.';
COMMENT ON TABLE scorecard_promise IS 'Public promises vs delivery for a mandate; owned by dsoc-scorecard.';

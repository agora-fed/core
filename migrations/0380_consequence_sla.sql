-- 0380_consequence_sla — owned by `dsoc-consequence` (range 0380, see migrations/REGISTRY.md).
-- The consequence / accountability thesis (PLAN.md section 4, Decidim failure #1): when a clustered
-- proposal crosses its directed support threshold, an SLA clock starts against the targeted official;
-- on expiry the platform PUBLICLY records silence. Explicit, auditable SQL (PLAN.md principle 3).
--
-- Cross-crate FKs target ONLY core identity tables (`org`, `mandate`). `cluster_id` and `proposal_id`
-- are bare uuids: the `consensus`/`proposals` tables are owned by other crates, so we never FK across
-- that boundary (ARCHITECTURE.md section 3 / migrations/REGISTRY.md).

-- One SLA clock per (proposal, cluster). The UNIQUE (proposal_id, cluster_id) constraint makes
-- consumption of `proposals.threshold.crossed` IDEMPOTENT: a duplicate threshold event (at-least-once
-- delivery) cannot start a second clock against the same official for the same demand.
CREATE TABLE consequence_sla (
    id           uuid PRIMARY KEY,
    org_id       uuid NOT NULL REFERENCES org(id),
    mandate_id   uuid NOT NULL REFERENCES mandate(id),
    cluster_id   uuid NOT NULL,
    proposal_id  uuid NOT NULL,
    started_at   timestamptz NOT NULL,
    due_at       timestamptz NOT NULL,
    status       text NOT NULL DEFAULT 'pending'
                 CHECK (status IN ('pending', 'answered', 'acted', 'ignored')),
    created_at   timestamptz NOT NULL,
    UNIQUE (proposal_id, cluster_id)
);
-- Drives the expiry sweep: find pending clocks whose due time has passed.
CREATE INDEX consequence_sla_status_due_idx ON consequence_sla (status, due_at);
-- Keyset pagination + per-org public surface listing.
CREATE INDEX consequence_sla_org_idx ON consequence_sla (org_id, id);

-- An official's response within (or before the recording of) the SLA window. Append-only history:
-- there may be more than one response row, but the SLA reaches a terminal state on the first.
CREATE TABLE consequence_response (
    id            uuid PRIMARY KEY,
    sla_id        uuid NOT NULL REFERENCES consequence_sla(id),
    mandate_id    uuid NOT NULL REFERENCES mandate(id),
    body          text NOT NULL,
    committed     boolean NOT NULL,
    responded_at  timestamptz NOT NULL,
    created_at    timestamptz NOT NULL
);
CREATE INDEX consequence_response_sla_idx ON consequence_response (sla_id);

-- The terminal, WRITE-ONCE public record for an SLA. The UNIQUE (sla_id) constraint makes the first
-- terminal decision permanent: once silence ('ignored') is recorded it can never be overwritten by a
-- late response, and once a response ('answered'/'acted') is recorded the sweep can never overwrite it
-- with silence. This is the heart of the thesis — silence is permanent and public.
CREATE TABLE consequence_outcome (
    id           uuid PRIMARY KEY,
    sla_id       uuid NOT NULL UNIQUE REFERENCES consequence_sla(id),
    outcome      text NOT NULL CHECK (outcome IN ('answered', 'acted', 'ignored')),
    decided_at   timestamptz NOT NULL,
    created_at   timestamptz NOT NULL
);

COMMENT ON TABLE consequence_sla IS 'Consequence SLA clocks; emits consequence.sla.started / consequence.sla.expired / consequence.official.responded (crates/core/src/events.rs).';
COMMENT ON TABLE consequence_outcome IS 'Write-once public accountability record: answered/acted/ignored. Silence is permanent.';

-- 0370_accountability_results — owned by `dsoc-accountability` (range 0370, see migrations/REGISTRY.md).
-- Accountability: the public record of RESULTS and PROGRESS of commitments made inside spaces.
-- Each result tracks one commitment toward a target; status updates are the append-only progress
-- ledger that drives the result's denormalised `progress`/`status`. Explicit, auditable SQL
-- (PLAN.md principle 3): no ORM, compile-time-checked by sqlx, no SELECT *. PostgreSQL 16+.
--
-- Cross-crate foreign keys point ONLY at the core identity table `org` (ARCHITECTURE.md section 3
-- / migrations/REGISTRY.md). The status-update FK stays inside this crate's own tables.

-- ---------------------------------------------------------------------------
-- accountability_result — one tracked commitment. `progress` is bounded 0..100 and `status` is a
-- denormalised lifecycle marker derived from progress: 0 -> pending, 1..99 -> in_progress,
-- 100 -> achieved. `achieved` is terminal: once reached, no further status updates are accepted
-- (the service guards the transition with the expected prior state).
-- ---------------------------------------------------------------------------
CREATE TABLE accountability_result (
    id          uuid PRIMARY KEY,
    -- Cross-crate FK to the core identity table `org`.
    org_id      uuid NOT NULL REFERENCES org(id),
    title       text NOT NULL CHECK (length(title) > 0),
    target      text NOT NULL,
    status      text NOT NULL CHECK (status IN ('pending', 'in_progress', 'achieved')),
    progress    integer NOT NULL CHECK (progress BETWEEN 0 AND 100),
    created_at  timestamptz NOT NULL,
    -- Lifecycle/progress consistency: status must agree with the progress bucket.
    CONSTRAINT accountability_result_status_progress_consistency CHECK (
        (status = 'pending'     AND progress = 0)
        OR (status = 'in_progress' AND progress BETWEEN 1 AND 99)
        OR (status = 'achieved'    AND progress = 100)
    )
);

-- Keyset pagination of an org's results, newest-first by (created_at, id).
CREATE INDEX accountability_result_org_idx ON accountability_result (org_id, created_at DESC, id DESC);

-- ---------------------------------------------------------------------------
-- accountability_status_update — the append-only progress ledger for a result. Each row records a
-- note and the progress value reported at that point; the parent result's denormalised
-- `progress`/`status` are updated in the SAME transaction as the insert.
-- ---------------------------------------------------------------------------
CREATE TABLE accountability_status_update (
    id          uuid PRIMARY KEY,
    result_id   uuid NOT NULL REFERENCES accountability_result(id),
    note        text NOT NULL CHECK (length(note) > 0),
    progress    integer NOT NULL CHECK (progress BETWEEN 0 AND 100),
    created_at  timestamptz NOT NULL
);

-- Keyset pagination of a result's status updates, newest-first by (created_at, id).
CREATE INDEX accountability_status_update_result_idx
    ON accountability_status_update (result_id, created_at DESC, id DESC);

COMMENT ON TABLE accountability_result IS
    'Tracked commitment with bounded progress and a derived lifecycle status; owned by dsoc-accountability.';
COMMENT ON TABLE accountability_status_update IS
    'Append-only progress ledger for an accountability_result; owned by dsoc-accountability.';

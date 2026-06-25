-- 0310_votes_support — owned by `dsoc-votes` (Tier 2). Support signals on proposals with
-- privacy-preserving aggregation (LGPD). Explicit, auditable SQL (PLAN.md principle 3).
-- Cross-crate FKs target ONLY core identity tables (org, citizen) per migrations/REGISTRY.md.

-- ---------------------------------------------------------------------------
-- votes_vote — the PROTECTED per-citizen support linkage. This is the LGPD-sensitive
-- table: it records *which citizen* supported *which proposal*. It is NEVER exposed via
-- any official-facing query (officials read only the aggregate `votes_vote_tally`).
-- The UNIQUE(proposal_id, citizen_id) constraint enforces one support signal per citizen
-- per proposal (a second cast is a Conflict, not a double-count).
-- ---------------------------------------------------------------------------
CREATE TABLE votes_vote (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES org(id),
    -- The proposal supported. Held as a bare uuid (no cross-crate FK to `proposals`):
    -- the crate-isolation rule forbids FKs to another component crate's tables.
    proposal_id uuid NOT NULL,
    -- The supporting citizen — the protected linkage. FK to the core identity table only.
    citizen_id  uuid NOT NULL REFERENCES citizen(id),
    created_at  timestamptz NOT NULL,
    UNIQUE (proposal_id, citizen_id)
);

-- ---------------------------------------------------------------------------
-- votes_vote_tally — the privacy-safe AGGREGATE officials may read. It carries only the
-- proposal id and a running support count: no citizen linkage can be derived from it.
-- The official-facing service path reads ONLY this table.
-- ---------------------------------------------------------------------------
CREATE TABLE votes_vote_tally (
    proposal_id   uuid PRIMARY KEY,
    support_count bigint NOT NULL DEFAULT 0 CHECK (support_count >= 0),
    updated_at    timestamptz NOT NULL
);

-- Keyset-friendly ordering for paginated aggregate listings (officials browse tallies).
CREATE INDEX votes_vote_tally_proposal_idx ON votes_vote_tally (proposal_id);

COMMENT ON TABLE votes_vote IS 'dsoc-votes: PROTECTED per-citizen support linkage (LGPD); never exposed to officials.';
COMMENT ON TABLE votes_vote_tally IS 'dsoc-votes: privacy-safe aggregate support count; the only official-facing surface.';

-- 0300_proposals_tables — owned by `dsoc-proposals` (range 0300, see migrations/REGISTRY.md).
-- Proposals: the primary civic artifact citizens create, directed at a mandate. This crate owns the
-- threshold logic that triggers the consequence loop (the accountability thesis). Explicit,
-- auditable SQL (PLAN.md principle 3).
--
-- Cross-crate FKs target ONLY core identity tables (org, citizen, mandate). `cluster_id` is a bare
-- uuid SOFT reference: the consensus cluster table is owned by another crate, so we never FK across
-- that boundary (ARCHITECTURE.md section 3). `proposal_revision.proposal_id` is an INTRA-crate FK to
-- our own `proposal` table.

-- The proposal. `support_count` is maintained from the event stream (votes.tally.updated); this
-- crate never reads the votes tables. `threshold_crossed_at` is the idempotency guard for the
-- consequence loop: it is set exactly once, and only when the proposal belongs to a consensus
-- cluster — so duplicate vote signals can never re-fire the one accountability signal.
CREATE TABLE proposal (
    id                   uuid PRIMARY KEY,
    org_id               uuid NOT NULL REFERENCES org(id),
    -- The mandate this proposal is directed at (drives the consequence loop).
    mandate_id           uuid NOT NULL REFERENCES mandate(id),
    -- Consensus cluster this proposal was merged into, if any. Soft ref (NO cross-crate FK).
    cluster_id           uuid,
    title                text NOT NULL,
    body                 text NOT NULL,
    status               text NOT NULL DEFAULT 'draft'
                         CHECK (status IN ('draft', 'published', 'clustered')),
    -- Aggregate support tally, maintained from votes.tally.updated events (never per-citizen).
    support_count        bigint NOT NULL DEFAULT 0,
    -- Support count at which the consequence loop fires.
    threshold            integer NOT NULL,
    -- Set exactly once when support crosses the threshold WHILE clustered (idempotency guard).
    threshold_crossed_at timestamptz,
    -- Set when the proposal cleared moderation and was published.
    published_at         timestamptz,
    created_at           timestamptz NOT NULL
);
-- Keyset pagination over ascending id (UUIDv7 = time-ordered), scoped by org.
CREATE INDEX proposal_org_idx ON proposal (org_id, id);

-- Append-only revision history: every edit inserts a new row; existing rows are never updated or
-- deleted, preserving the full provenance trail an accountability platform requires.
CREATE TABLE proposal_revision (
    id           uuid PRIMARY KEY,
    proposal_id  uuid NOT NULL REFERENCES proposal(id),
    title        text NOT NULL,
    body         text NOT NULL,
    -- The citizen who authored this revision (core identity table).
    edited_by    uuid NOT NULL REFERENCES citizen(id),
    created_at   timestamptz NOT NULL
);
CREATE INDEX proposal_revision_proposal_idx ON proposal_revision (proposal_id, id);

COMMENT ON TABLE proposal IS 'Proposals; emits proposals.created / proposals.published / proposals.threshold.crossed (crates/core/src/events.rs).';
COMMENT ON TABLE proposal_revision IS 'Append-only proposal edit history (preserve provenance).';

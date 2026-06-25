-- 0320_comments_threads — owned by `dsoc-comments`.
-- Threaded deliberation on any votable entity (proposals). Explicit SQL,
-- compile-time-checked by sqlx (PLAN.md principle 3 — no ORM). PostgreSQL 16+.
--
-- Cross-crate foreign keys point ONLY at the core identity tables `org` and
-- `citizen` (ARCHITECTURE.md section 3 / migrations/REGISTRY.md). The self-referential
-- FK on `comment.parent_id` targets a table OWNED BY THIS CRATE, which is allowed.
-- The `proposal_id` is stored as a bare uuid: comments never reach into the
-- proposals crate's tables.

-- ---------------------------------------------------------------------------
-- comment — one node in a deliberation thread. A root comment has `parent_id`
-- NULL and `depth` 0; a reply points at its parent (same proposal) and carries
-- `depth = parent.depth + 1`. The denormalized `depth` makes the max-thread-depth
-- guard a single read and keeps depth consistently available on every read path.
-- `status` is a small lifecycle: visible -> flagged | hidden (moderation).
-- ---------------------------------------------------------------------------
CREATE TABLE comment (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES org(id),
    -- The votable entity (a proposal) this thread hangs off. Bare uuid by design:
    -- comments never FK into another crate's tables.
    proposal_id uuid NOT NULL,
    -- Self-referential parent (same crate -> FK allowed). NULL for a root comment.
    parent_id   uuid REFERENCES comment(id),
    -- Author; cross-crate FK to the core identity table `citizen` (allowed).
    author_id   uuid NOT NULL REFERENCES citizen(id),
    body        text NOT NULL CHECK (length(btrim(body)) > 0),
    -- Denormalized thread depth (0 for a root); guarded in the domain at insert time.
    depth       integer NOT NULL DEFAULT 0 CHECK (depth >= 0),
    status      text NOT NULL DEFAULT 'visible'
                CHECK (status IN ('visible', 'flagged', 'hidden')),
    created_at  timestamptz NOT NULL
);

-- Keyset pagination of a proposal's thread, oldest-first by (created_at, id) so a
-- reply always sorts after its parent within a page.
CREATE INDEX comment_thread_idx
    ON comment (org_id, proposal_id, created_at, id);
-- Fetch the direct replies of a comment.
CREATE INDEX comment_parent_idx
    ON comment (parent_id);
-- Drive the moderation fan-out: flag the still-visible comments of a proposal.
CREATE INDEX comment_proposal_status_idx
    ON comment (proposal_id, status);

-- ---------------------------------------------------------------------------
-- comment_vote — a citizen's up/down weight on a single comment. At most one row
-- per (comment, citizen): a re-vote is an idempotent upsert that updates the
-- weight, never a duplicate. `weight` is constrained to the unit set {-1, +1}.
-- ---------------------------------------------------------------------------
CREATE TABLE comment_vote (
    id          uuid PRIMARY KEY,
    comment_id  uuid NOT NULL REFERENCES comment(id),
    citizen_id  uuid NOT NULL REFERENCES citizen(id),
    weight      smallint NOT NULL CHECK (weight IN (-1, 1)),
    created_at  timestamptz NOT NULL,
    UNIQUE (comment_id, citizen_id)
);

-- Aggregate a comment's score / list a citizen's votes.
CREATE INDEX comment_vote_comment_idx
    ON comment_vote (comment_id);

COMMENT ON TABLE comment IS 'Threaded deliberation node; owned by dsoc-comments.';
COMMENT ON TABLE comment_vote IS 'One up/down vote per citizen per comment; owned by dsoc-comments.';

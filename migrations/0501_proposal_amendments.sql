-- Migration 0501 — proposal_amendment (Decidim gap parity).
--
-- Amendments are Decidim-style forks of a proposal: another citizen offers a
-- variant of the text with a rationale. Each amendment can be voted on
-- independently and, when accepted by the original author (or a moderator
-- once the platform grows), replaces the proposal body in a new revision.
--
-- We keep the record even after acceptance/rejection so the history is
-- auditable — a hallmark of Decidim's transparency model.

BEGIN;

CREATE TABLE proposal_amendment (
    id            uuid PRIMARY KEY,
    proposal_id   uuid NOT NULL REFERENCES proposal(id) ON DELETE CASCADE,
    -- The citizen who authored the amendment. Core identity table; safe cross-crate FK.
    author_id     uuid NOT NULL REFERENCES citizen(id),
    -- The full replacement text. Same size envelope as proposal.body.
    body          text NOT NULL CHECK (length(body) BETWEEN 1 AND 20000),
    -- Why: the author's justification for the change. Optional.
    rationale     text CHECK (rationale IS NULL OR length(rationale) BETWEEN 1 AND 4000),
    -- Lifecycle:
    --   draft      — author is still editing, not visible to others.
    --   open       — visible + accepting votes.
    --   accepted   — original author (or an admin) accepted; body was applied
    --                as a new proposal_revision.
    --   rejected   — original author declined the change.
    --   withdrawn  — the amendment author took it back.
    status        text NOT NULL DEFAULT 'draft'
                  CHECK (status IN ('draft','open','accepted','rejected','withdrawn')),
    -- Aggregate support tally (independent from the proposal's tally). Kept
    -- monotonic via the same event-driven pattern as `proposal.support_count`.
    support_count bigint NOT NULL DEFAULT 0,
    created_at    timestamptz NOT NULL DEFAULT now(),
    -- Set when status transitions out of `draft`.
    published_at  timestamptz,
    -- Set when status becomes accepted/rejected/withdrawn.
    resolved_at   timestamptz
);

CREATE INDEX proposal_amendment_proposal_idx
    ON proposal_amendment (proposal_id, status, created_at DESC);
CREATE INDEX proposal_amendment_author_idx
    ON proposal_amendment (author_id, created_at DESC);

COMMENT ON TABLE proposal_amendment IS
    '0.20.0-decidim: Decidim-parity amendments — variant text + rationale + lifecycle.';

COMMIT;

-- 0301_proposal_author — record who proposed each demand (ADR-0010 P2). Until now `proposal` had
-- no author column; the front showed an orphan title with no human attribution, which weakened the
-- accountability narrative ("alguém propôs" vs "@fulana propôs"). Range 0300-0309 = proposals
-- per REGISTRY.md.
--
-- Nullable: existing rows (seeded from earlier sessions) have no known author, and any future
-- platform-generated proposal can stay anonymous. The DTO presents this as "Anônima" in the UI.

ALTER TABLE proposal
    ADD COLUMN author_citizen_id uuid REFERENCES citizen(id);

-- Lookups by author for "minhas propostas" panels later. Partial so the index stays compact when
-- many rows are anonymous.
CREATE INDEX proposal_author_idx
    ON proposal (author_citizen_id, created_at DESC)
 WHERE author_citizen_id IS NOT NULL;

COMMENT ON COLUMN proposal.author_citizen_id IS
    'Citizen who created this proposal. NULL = anonymous / platform-seeded (rendered as "Anônima").';

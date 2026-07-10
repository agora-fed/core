-- 0518_text_sample.sql
--
-- NLI pair judge (0.28.0): the merge gate now reads the candidate pair
-- JOINTLY (cross-encoder), which requires the raw text of cluster members —
-- crate-owned derived copy stored at ingest (first ~1200 chars), exactly like
-- `direction_signature` (0517). Backfill: existing rows keep '' (the judge
-- treats an empty sample as "no opinion" and the cheaper guards decide).

ALTER TABLE consensus_embedding
    ADD COLUMN IF NOT EXISTS text_sample text NOT NULL DEFAULT '';

COMMENT ON COLUMN consensus_embedding.text_sample IS
    '0.28.0: proposal text sample (~1200 chars) for the NLI merge judge (nli_judge.rs).';

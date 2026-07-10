-- 0517_direction_signature.sql
--
-- Stance guard (0.27.1): `consensus_embedding` carries the proposal's
-- policy-DIRECTION signature (crate-owned derived data, computed by
-- crates/platform/consensus/src/stance.rs at ingest). At merge time the
-- service compares the candidate's signature against the cluster members'
-- and VETOES the merge on antagonistic directions — measured on pt-BR pairs,
-- "privatizar o SUS" vs "proibir a privatização do SUS" sit at cosine 0.015,
-- below every legitimate paraphrase, so no distance threshold can separate
-- them; only direction can.
--
-- Entries: 'a:<axis><sign>' (policy axis stance), 'n:<stem>' (negated stem),
-- 's:<stem>' (asserted stem). Backfill: existing rows keep '{}' (no veto
-- signal) until the fatia-2 re-embed job recomputes them.

ALTER TABLE consensus_embedding
    ADD COLUMN IF NOT EXISTS direction_signature text[] NOT NULL DEFAULT '{}';

COMMENT ON COLUMN consensus_embedding.direction_signature IS
    '0.27.1: policy-direction signature (stance.rs) — merge veto for ideologically antagonistic proposals.';

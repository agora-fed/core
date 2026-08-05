-- 0520_candidacy_tse_sq.sql — the TSE's stable key for idempotent ingestion
-- (item 5 of the strategic plan: DivulgaCand pipeline ready for 2026-08-15).
--
-- During the registration window the TSE REPUBLISHES the consulta_cand CSVs
-- daily (candidacies come in, statuses change: granted/denied).
-- The pipeline must run every day without duplicating: `SQ_CANDIDATO` is the
-- unique per-candidacy identifier the TSE itself issues — it becomes the
-- upsert key. Old rows (example seeds) stay NULL and outside the partial
-- index.

ALTER TABLE candidacy
    ADD COLUMN IF NOT EXISTS tse_sq text;

CREATE UNIQUE INDEX IF NOT EXISTS candidacy_tse_sq_uidx
    ON candidacy (tse_sq)
    WHERE tse_sq IS NOT NULL;

COMMENT ON COLUMN candidacy.tse_sq IS
    '0.29: SQ_CANDIDATO do TSE (consulta_cand) — chave de upsert da ingestão oficial.';

-- 0520_candidacy_tse_sq.sql — chave estável do TSE pra ingestão idempotente
-- (item 5 do plano estratégico: pipeline DivulgaCand pronto pra 15/08/2026).
--
-- Durante a janela de registro o TSE REPUBLICA os CSVs de consulta_cand
-- diariamente (candidaturas entram, situações mudam: deferida/indeferida).
-- O pipeline precisa rodar todo dia sem duplicar: `SQ_CANDIDATO` é o
-- identificador único por candidatura que o próprio TSE emite — vira a
-- chave de upsert. Rows antigas (seeds de exemplo) ficam com NULL e fora
-- do índice parcial.

ALTER TABLE candidacy
    ADD COLUMN IF NOT EXISTS tse_sq text;

CREATE UNIQUE INDEX IF NOT EXISTS candidacy_tse_sq_uidx
    ON candidacy (tse_sq)
    WHERE tse_sq IS NOT NULL;

COMMENT ON COLUMN candidacy.tse_sq IS
    '0.29: SQ_CANDIDATO do TSE (consulta_cand) — chave de upsert da ingestão oficial.';

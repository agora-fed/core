-- Migration 0105 — título de eleitor no cidadão (verificação política).
--
-- Cria a coluna `titulo_eleitor` em `citizen` que atesta ser cidadã(o)
-- brasileira(o) apta(o) a votar (título válido no TSE). Junto com
-- `titulo_status` — algorítmico (dígitos verificadores) ou verified
-- (cross-check com fonte oficial futura, e.g. Serpro/TSE dados abertos).
--
-- Regra: só cidadã(o) com `titulo_status = 'validated'` ou 'verified' pode
-- votar em pauta urgente (Fatia D) — separa participação civil (qualquer
-- cidadã(o)) de decisão vinculante (cidadã(o) verificada(o) apta a votar
-- no Brasil real).
--
-- Idempotente: rerun-safe.

BEGIN;

ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS titulo_eleitor text,
    ADD COLUMN IF NOT EXISTS titulo_status text
        CHECK (titulo_status IS NULL OR titulo_status IN
               ('unverified','validated','verified'));

-- UNIQUE parcial: mesmo título não pode aparecer em duas contas (bloqueio
-- básico contra sock-puppet). NULL não colide entre si (WHERE cláusula).
CREATE UNIQUE INDEX IF NOT EXISTS citizen_titulo_eleitor_unique
    ON citizen (titulo_eleitor)
    WHERE titulo_eleitor IS NOT NULL;

COMMENT ON COLUMN citizen.titulo_eleitor IS
    '0.25.0-fediverso: 12 dígitos do título de eleitor TSE (formato SEQ + UF + DVs).';
COMMENT ON COLUMN citizen.titulo_status IS
    '0.25.0-fediverso: unverified | validated (dígitos OK) | verified (cross-check TSE).';

COMMIT;

-- Migration 0302 — nível de urgência da proposta (Fatia D — 0.25.0-fediverso).
--
-- Marca a proposta como 'comum' (default) ou 'urgente'. O gate de voto no
-- caso urgente exige `citizen.titulo_status ∈ ('validated','verified')` —
-- separação entre participação civil (todo cidadão) e decisão vinculante
-- (cidadão comprovadamente apto a votar no Brasil real).
--
-- Aditivo + backward-compat: propostas existentes ficam 'comum'; nenhum
-- fluxo antigo quebra. A UX no front destaca o badge 🔥 URGENTE.

BEGIN;

ALTER TABLE proposal
    ADD COLUMN IF NOT EXISTS urgencia text NOT NULL DEFAULT 'comum'
        CHECK (urgencia IN ('comum','urgente'));

COMMENT ON COLUMN proposal.urgencia IS
    '0.25.0-fediverso: nível de urgência. urgente ⇒ voto exige titulo_status validated/verified.';

-- Owner alignment (mesmo motivo de 0106/0107 — migrations rodam como postgres em prod).
-- Aqui só ALTER COLUMN, então o ownership da tabela original é preservado.

COMMIT;

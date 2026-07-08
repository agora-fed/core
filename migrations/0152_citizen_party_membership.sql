-- Migration 0152 — filiação partidária opcional do cidadão comum.
--
-- Contexto: até aqui a plataforma diferenciava só:
--   - mandate.party  → partido do político no mandato
--   - party_administrator → cidadão que administra um partido
-- Faltava o caso mais comum: cidadã(o) comum simplesmente diz "sou do PT",
-- sem virar admin, sem ter mandato. Sinaliza pra o restante da UI ("meu
-- partido") + serve pra scorecard/filtros por partido no admin.
--
-- Nullable — a filiação é opcional. FK soft (só valida se preencher): assim
-- adicionar/remover partidos oficialmente não quebra dados históricos.

BEGIN;

-- Sem FK: party PK é (org_id, sigla), e forçar isso na citizen exigiria
-- carregar o org_id da citizen no INSERT/UPDATE (a UI faz join). A validação
-- prática vem do dropdown do front (lê /parties) e da consistência ORG-level
-- que a plataforma já mantém.
ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS party_sigla text;

CREATE INDEX IF NOT EXISTS citizen_party_sigla_idx
    ON citizen (party_sigla)
    WHERE party_sigla IS NOT NULL;

COMMENT ON COLUMN citizen.party_sigla IS
    '0.25.0-fediverso: filiação partidária opcional (só informativo, sem permissões). FK em party.sigla.';

COMMIT;

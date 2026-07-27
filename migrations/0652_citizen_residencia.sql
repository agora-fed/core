-- 0652_citizen_residencia.sql — domicílio declarado do cidadão (UF + município IBGE).
--
-- Coletado OBRIGATORIAMENTE no cadastro (front + backend exigem). É o eixo
-- territorial do projeto — escopo de sorteio/federação municipal e estadual —,
-- independente do título de eleitor (opcional e esparso). Nullable no schema
-- (linhas antigas ficam sem; backfill via nudge de perfil); a obrigatoriedade
-- é imposta na borda do cadastro, não no banco.
--
-- `uf` é denormalizado de `municipio_ibge` para filtro rápido por estado.
-- Idempotente: rerun-safe.

BEGIN;

ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS uf text
        CHECK (uf IS NULL OR uf ~ '^[A-Z]{2}$'),
    ADD COLUMN IF NOT EXISTS municipio_ibge integer
        REFERENCES municipio_ibge (codigo_ibge);

COMMENT ON COLUMN citizen.uf IS
    '0652: UF de domicílio (obrigatória na borda do cadastro; denormalizada de municipio_ibge).';
COMMENT ON COLUMN citizen.municipio_ibge IS
    '0652: município de domicílio (FK municipio_ibge.codigo_ibge), declarado no cadastro.';

COMMIT;

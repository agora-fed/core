-- 0653_pending_signup_residencia.sql — carrega o domicílio (UF + município IBGE)
-- na pending de cadastro, para o `confirm` materializá-lo no citizen (0652).
--
-- O cidadão só vira `citizen` ao confirmar o e-mail; o domicílio informado no
-- request precisa sobreviver até lá. Espelha o padrão do `candidate_meta` (0526):
-- coletado no request, aplicado no confirm. Só relevante para role='cidadao'.
--
-- Idempotente: rerun-safe.

BEGIN;

ALTER TABLE auth_pending_signup
    ADD COLUMN IF NOT EXISTS residencia_uf text
        CHECK (residencia_uf IS NULL OR residencia_uf ~ '^[A-Z]{2}$'),
    ADD COLUMN IF NOT EXISTS residencia_municipio_ibge integer;

COMMENT ON COLUMN auth_pending_signup.residencia_uf IS
    '0653: UF de domicílio informada no cadastro do cidadão; aplicada no confirm.';
COMMENT ON COLUMN auth_pending_signup.residencia_municipio_ibge IS
    '0653: código IBGE do município de domicílio informado no cadastro; aplicado no confirm.';

COMMIT;

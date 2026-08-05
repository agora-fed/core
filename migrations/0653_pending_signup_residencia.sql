-- 0653_pending_signup_residencia.sql — carries the domicile (state + IBGE municipality)
-- on the pending signup, so `confirm` materialises it on the citizen (0652).
--
-- A citizen only becomes a `citizen` once the e-mail is confirmed; the domicile given in
-- the request must survive until then. Mirrors the `candidate_meta` pattern (0526):
-- collected on request, applied on confirm. Relevant only for role='cidadao'.
--
-- Idempotent: rerun-safe.

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

-- 0664_signup_personal_fields.sql — dados pessoais obrigatórios no cadastro (ÁGORA).
--
-- Diretriz do Marcos (2026-07-28): Nome, Sobrenome, Nick (fediverso), Sexo, CPF, Data de
-- nascimento, Estado e Município são OBRIGATÓRIOS no cadastro do cidadão; só o título é opcional.
-- Hoje o form coleta nome/nascimento/sexo mas o backend os DESCARTA (o `auth_pending_signup` não
-- tinha colunas), e o nick nunca era coletado (handle auto). Esta migração dá onde guardar:
--
-- - `citizen.birth_date` — data de nascimento (não havia coluna).
-- - `auth_pending_signup.{full_name,gender,birth_date,handle}` — carregam do cadastro até o
--   `signup_verify` materializar o `citizen`. `gender` reusa o vocabulário de `citizen.gender`
--   (mulher|homem|nao-binarie|prefiro-nao-dizer); o form mapeia sexo F→mulher, M→homem.
--
-- Idempotente: rerun-safe.

BEGIN;

ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS birth_date date;

ALTER TABLE auth_pending_signup
    ADD COLUMN IF NOT EXISTS full_name  text,
    ADD COLUMN IF NOT EXISTS gender     text,
    ADD COLUMN IF NOT EXISTS birth_date date,
    ADD COLUMN IF NOT EXISTS handle     text;

COMMENT ON COLUMN citizen.birth_date IS
    '0664: data de nascimento do cidadão (obrigatória no cadastro; usada tb na verificação de CPF).';

COMMIT;

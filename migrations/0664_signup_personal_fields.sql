-- 0664_signup_personal_fields.sql — mandatory personal data at signup (AGORA).
--
-- Product decision (2026-07-28): first name, last name, nick (fediverse handle), gender, CPF,
-- birth date, state and municipality are MANDATORY at citizen signup; only the voter ID is optional.
-- Today the form collects name/birth date/gender but the backend DISCARDS them (`auth_pending_signup`
-- had no columns), and the nick was never collected (auto handle). This migration adds the storage:
--
-- - `citizen.birth_date` — date of birth (there was no column).
-- - `auth_pending_signup.{full_name,gender,birth_date,handle}` — carried from signup until
--   `signup_verify` materialises the `citizen`. `gender` reuses the `citizen.gender` vocabulary
--   (mulher|homem|nao-binarie|prefiro-nao-dizer); the form maps F→mulher, M→homem.
--
-- Idempotent: rerun-safe.

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

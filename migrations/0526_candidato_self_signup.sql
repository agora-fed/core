-- Migration 0526 — cadastro de candidato(a) sem mandato (auto-declarado).
--
-- Hoje os dois caminhos de político (mandate_invite e register_politician)
-- exigem uma row de `mandate` pré-existente com public_email conferível —
-- quem ainda não tem mandato fica fora. ADR-0005 já decide a arquitetura:
-- "voter → candidate → official" é a MESMA identidade de Ator evoluindo.
-- Esta migration abre os três encaixes que o fluxo novo usa:
--
--   1. `mandate.source = 'self'` — mandato criado pelo próprio candidato no
--      cadastro (is_candidate=true, nunca setado por seed até aqui).
--   2. `auth_pending_signup.role = 'candidato'` + `candidate_meta` jsonb —
--      o request guarda nome de urna/cargo/UF/partido; o confirm materializa
--      mandate + binding + candidacy numa tx (mesmo pattern do politico).
--   3. `candidacy.listed` — decisão de produto 2026-07-24: autodeclarado NÃO
--      entra no comparador público de /eleicoes até verificação (atestação
--      de partido/mandato, match TSE ou admin). Import TSE continua listado
--      (DEFAULT true); o self-signup insere `listed = false`.
--
-- O binding do candidato nasce `verification_level = 'email'` (autodeclarado)
-- — nunca 'directory' como os fluxos com prova de e-mail oficial.

BEGIN;

-- 1. mandate.source aceita 'self' (mesmo pattern do 0503).
ALTER TABLE mandate
    DROP CONSTRAINT IF EXISTS mandate_source_check;
ALTER TABLE mandate
    ADD CONSTRAINT mandate_source_check
    CHECK (source IS NULL OR source IN ('camara', 'senado', 'assembleia', 'tse', 'manual', 'self'));

COMMENT ON COLUMN mandate.source IS
    '0.36.0: origem da row. assembleia=feed estadual; tse=CSV de eleitos; self=cadastro do próprio candidato (is_candidate=true).';

-- 2. pending signup: papel 'candidato' + metadados da candidatura.
ALTER TABLE auth_pending_signup
    ADD COLUMN candidate_meta jsonb;

ALTER TABLE auth_pending_signup
    DROP CONSTRAINT IF EXISTS auth_pending_signup_role_check;
ALTER TABLE auth_pending_signup
    ADD CONSTRAINT auth_pending_signup_role_check
    CHECK (role IN ('cidadao', 'politico', 'candidato'));

-- Consistência papel/payload (substitui o CHECK anônimo da 0106):
-- politico ⇒ mandate_id; candidato ⇒ candidate_meta.
ALTER TABLE auth_pending_signup
    DROP CONSTRAINT IF EXISTS auth_pending_signup_check;
ALTER TABLE auth_pending_signup
    ADD CONSTRAINT auth_pending_signup_role_payload_check
    CHECK (
        (role <> 'politico' OR mandate_id IS NOT NULL)
        AND (role <> 'candidato' OR candidate_meta IS NOT NULL)
    );

COMMENT ON COLUMN auth_pending_signup.candidate_meta IS
    '0.36.0: role=candidato — {display_name, office, sphere, uf, municipio, party_sigla, number}. Validado no request; o confirm só materializa.';

-- 3. candidacy.listed — vitrine do comparador. TSE/backfill ficam listados
-- (default true); autodeclarado entra false até verificação.
ALTER TABLE candidacy
    ADD COLUMN listed boolean NOT NULL DEFAULT true;

COMMENT ON COLUMN candidacy.listed IS
    '0.36.0: aparece no comparador público? false = candidatura autodeclarada ainda não verificada (partido/TSE/admin).';

COMMIT;

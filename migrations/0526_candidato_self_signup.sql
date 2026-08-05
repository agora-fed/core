-- Migration 0526 — signup of a candidate without a mandate (self-declared).
--
-- Today both official paths (mandate_invite and register_politician)
-- require a pre-existing `mandate` row with a checkable public_email —
-- whoever has no mandate yet is left out. ADR-0005 already decides the architecture:
-- "voter → candidate → official" is the SAME Actor identity evolving.
-- This migration opens the three sockets the new flow uses:
--
--   1. `mandate.source = 'self'` — a mandate created by the candidate themselves at
--      signup (is_candidate=true, never set by a seed until now).
--   2. `auth_pending_signup.role = 'candidato'` + `candidate_meta` jsonb —
--      the request holds ballot name/office/state/party; the confirm materialises
--      mandate + binding + candidacy in one tx (the same pattern as politico).
--   3. `candidacy.listed` — product decision 2026-07-24: a self-declared one does NOT
--      enter the public /eleicoes comparator until verification (attestation
--      party/mandate, a TSE match or an admin). A TSE import stays listed
--      (DEFAULT true); o self-signup insere `listed = false`.
--
-- O binding do candidato nasce `verification_level = 'email'` (autodeclarado)
-- — never 'directory' like the flows with proof of an official e-mail.

BEGIN;

-- 1. mandate.source accepts 'self' (the same pattern as 0503).
ALTER TABLE mandate
    DROP CONSTRAINT IF EXISTS mandate_source_check;
ALTER TABLE mandate
    ADD CONSTRAINT mandate_source_check
    CHECK (source IS NULL OR source IN ('camara', 'senado', 'assembleia', 'tse', 'manual', 'self'));

COMMENT ON COLUMN mandate.source IS
    '0.36.0: origem da row. assembleia=feed estadual; tse=CSV de eleitos; self=cadastro do próprio candidato (is_candidate=true).';

-- 2. pending signup: role 'candidato' + the candidacy metadata.
ALTER TABLE auth_pending_signup
    ADD COLUMN candidate_meta jsonb;

ALTER TABLE auth_pending_signup
    DROP CONSTRAINT IF EXISTS auth_pending_signup_role_check;
ALTER TABLE auth_pending_signup
    ADD CONSTRAINT auth_pending_signup_role_check
    CHECK (role IN ('cidadao', 'politico', 'candidato'));

-- role/payload consistency (replaces 0106's anonymous CHECK):
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
-- (default true); a self-declared one starts false until verification.
ALTER TABLE candidacy
    ADD COLUMN listed boolean NOT NULL DEFAULT true;

COMMENT ON COLUMN candidacy.listed IS
    '0.36.0: aparece no comparador público? false = candidatura autodeclarada ainda não verificada (partido/TSE/admin).';

COMMIT;

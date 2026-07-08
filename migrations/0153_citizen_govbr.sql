-- Migration 0153 — nome legal (CPF/gov.br) + vínculo com gov.br OIDC.
--
-- Contexto: hoje `citizen.display_name` é o rótulo público livre que o
-- cidadão escolhe. Falta o nome jurídico "como consta no CPF/RG" — vindo
-- de fonte oficial (gov.br) quando o cidadão fizer login federado.
--
-- Separação intencional:
--   - `display_name`: público, livre, editável (aparece na UI).
--   - `legal_name`: preenchido só via gov.br (nunca editável pelo user).
--   - `govbr_sub`: identificador único opaco do gov.br (é o `sub` do OIDC).
--     Recebemos o CPF via escopo `cpf` também, mas o `sub` é o que assina.
--   - `govbr_confiabilidade`: 'bronze'|'prata'|'ouro' — nível de autenticação
--     que o gov.br atribuiu (biometria, digital, etc.).
--
-- Backend usa legal_name pro admin (GUI de usuários) e pra e-mails
-- ("Prezado(a) João da Silva"). A UI pública mantém `display_name` +
-- `@handle` — o cidadão nunca vê seu próprio legal_name exposto salvo
-- em Configurações "Nome oficial (via gov.br)".

BEGIN;

ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS legal_name text,
    ADD COLUMN IF NOT EXISTS govbr_sub text,
    ADD COLUMN IF NOT EXISTS govbr_confiabilidade text
        CHECK (govbr_confiabilidade IS NULL
               OR govbr_confiabilidade IN ('bronze','prata','ouro')),
    ADD COLUMN IF NOT EXISTS govbr_linked_at timestamptz;

-- govbr_sub é único por cidadão (não pode 1 CPF gov.br apontar pra 2 contas).
CREATE UNIQUE INDEX IF NOT EXISTS citizen_govbr_sub_unique
    ON citizen (govbr_sub)
    WHERE govbr_sub IS NOT NULL;

COMMENT ON COLUMN citizen.legal_name IS
    '0.25.0-fediverso: nome como consta no CPF (via gov.br). Nunca exposto na UI pública.';
COMMENT ON COLUMN citizen.govbr_sub IS
    '0.25.0-fediverso: identificador OIDC do gov.br (sub). Único por cidadão.';
COMMENT ON COLUMN citizen.govbr_confiabilidade IS
    'Nível gov.br: bronze (senha), prata (2fa), ouro (biometria/digital).';

COMMIT;

-- Migration 0153 — legal name (identity document/gov.br) + the gov.br OIDC binding.
--
-- Context: today `citizen.display_name` is the free public label the
-- citizen chooses. What is missing is the legal name "as it appears on the official document" — coming
-- from an official source (gov.br) when the citizen performs a federated login.
--
-- The separation is intentional:
--   - `display_name`: public, free, editable (appears in the UI).
--   - `legal_name`: filled only via gov.br (never editable by the user).
--   - `govbr_sub`: gov.br's opaque unique identifier (the OIDC `sub`).
--     We also receive the document number via the `cpf` scope, but the `sub` is what signs.
--   - `govbr_confiabilidade`: 'bronze'|'prata'|'ouro' — the authentication level
--     gov.br assigned (biometrics, fingerprint, etc.).
--
-- The backend uses legal_name for the admin (user GUI) and for e-mails
-- ("Dear João da Silva"). The public UI keeps `display_name` +
-- `@handle` — the citizen never sees their own legal_name exposed except
-- in Settings under "Official name (via gov.br)".

BEGIN;

ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS legal_name text,
    ADD COLUMN IF NOT EXISTS govbr_sub text,
    ADD COLUMN IF NOT EXISTS govbr_confiabilidade text
        CHECK (govbr_confiabilidade IS NULL
               OR govbr_confiabilidade IN ('bronze','prata','ouro')),
    ADD COLUMN IF NOT EXISTS govbr_linked_at timestamptz;

-- govbr_sub is unique per citizen (one gov.br identity cannot point at 2 accounts).
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

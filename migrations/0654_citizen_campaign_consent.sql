-- 0654_citizen_campaign_consent.sql — the citizen's campaign consent (F2, #59).
--
-- LGPD art. 11 (political opinion/affiliation = sensitive data): SPECIFIC and prominent
-- consent, **default OFF**. 4 levels of reach — the citizen authorizes their record to be
-- used for campaign communication by:
--   all_parties  : any directory of any party
--   party        : every directory of ONE party (party_sigla)
--   municipality : every party of ONE municipality (uf + municipio)
--   directory    : one directory of one municipality (party_sigla + uf + municipio)
--
-- Each row is a grant; multiple grants add up. Revocable (revoked_at). Resolving
-- "whom a directory reaches" belongs to phase F3 (broadcast), which crosses these grants with the
-- citizen's residence (0652). It never exports a raw list — sending is mediated.
--
-- Idempotente: rerun-safe.

BEGIN;

CREATE TABLE IF NOT EXISTS citizen_campaign_consent (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id       uuid NOT NULL REFERENCES org(id),
    citizen_id   uuid NOT NULL REFERENCES citizen(id),
    scope        text NOT NULL
                 CHECK (scope IN ('all_parties', 'party', 'municipality', 'directory')),
    party_sigla  text,        -- set for 'party' and 'directory'
    uf           text CHECK (uf IS NULL OR uf ~ '^[A-Z]{2}$'),
    municipio    text,        -- set for 'municipality' and 'directory'
    granted_at   timestamptz NOT NULL DEFAULT now(),
    revoked_at   timestamptz,

    CONSTRAINT campaign_consent_shape CHECK (
        (scope = 'all_parties'  AND party_sigla IS NULL     AND uf IS NULL     AND municipio IS NULL) OR
        (scope = 'party'        AND party_sigla IS NOT NULL AND uf IS NULL     AND municipio IS NULL) OR
        (scope = 'municipality' AND party_sigla IS NULL     AND uf IS NOT NULL AND municipio IS NOT NULL) OR
        (scope = 'directory'    AND party_sigla IS NOT NULL AND uf IS NOT NULL AND municipio IS NOT NULL)
    )
);

-- "my active consents" (the citizen's screen).
CREATE INDEX IF NOT EXISTS citizen_campaign_consent_citizen_idx
    ON citizen_campaign_consent (citizen_id)
    WHERE revoked_at IS NULL;

-- reach resolution by (scope, party, uf, municipio) — used in F3.
CREATE INDEX IF NOT EXISTS citizen_campaign_consent_reach_idx
    ON citizen_campaign_consent (scope, party_sigla, uf, municipio)
    WHERE revoked_at IS NULL;

COMMENT ON TABLE citizen_campaign_consent IS
    '0654 (F2/#59): consentimento de campanha do cidadão, 4 níveis, default OFF, revogável (LGPD art.11).';

COMMIT;

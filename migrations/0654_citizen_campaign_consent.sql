-- 0654_citizen_campaign_consent.sql — consentimento de campanha do cidadão (F2, #59).
--
-- LGPD art. 11 (opinião política/filiação = dados sensíveis): consentimento ESPECÍFICO e
-- destacado, **default OFF**. 4 níveis de capilaridade — o cidadão autoriza que sua base seja
-- usada para comunicação de campanha por:
--   all_parties  : qualquer diretório de qualquer partido
--   party        : todos os diretórios de UM partido (party_sigla)
--   municipality : todos os partidos de UM município (uf + municipio)
--   directory    : um diretório de um município (party_sigla + uf + municipio)
--
-- Cada linha é um grant; múltiplos grants somam capilaridade. Revogável (revoked_at). A
-- resolução "quem um diretório alcança" é da fase F3 (broadcast), que cruza estes grants com o
-- domicílio do cidadão (0652). Nunca exporta lista crua — envio mediado.
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

-- "meus consentimentos ativos" (tela do cidadão).
CREATE INDEX IF NOT EXISTS citizen_campaign_consent_citizen_idx
    ON citizen_campaign_consent (citizen_id)
    WHERE revoked_at IS NULL;

-- resolução de alcance por (scope, party, uf, municipio) — usada na F3.
CREATE INDEX IF NOT EXISTS citizen_campaign_consent_reach_idx
    ON citizen_campaign_consent (scope, party_sigla, uf, municipio)
    WHERE revoked_at IS NULL;

COMMENT ON TABLE citizen_campaign_consent IS
    '0654 (F2/#59): consentimento de campanha do cidadão, 4 níveis, default OFF, revogável (LGPD art.11).';

COMMIT;

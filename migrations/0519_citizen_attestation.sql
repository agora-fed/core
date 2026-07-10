-- 0519_citizen_attestation.sql — atestado de cidadania por operador
-- verificado (web-of-trust, 0.28.3). Enquanto não há verificação
-- institucional (TSE/gov.br indeferido em 2026-07-10), quem JÁ é
-- verificado — operador de mandato (mandate_identity_binding) ou admin
-- de partido aceito (party_administrator) — pode atestar publicamente
-- que conhece o cidadão. Auditável, revogável, selo no perfil público.
-- FK só para `citizen` (tabela de identidade core — regra do REGISTRY).

CREATE TABLE citizen_attestation (
    id                  uuid PRIMARY KEY,
    citizen_id          uuid NOT NULL REFERENCES citizen(id),
    attester_citizen_id uuid NOT NULL REFERENCES citizen(id),
    -- Poder que legitimou o atestado NO MOMENTO em que foi dado.
    attester_kind       text NOT NULL CHECK (attester_kind IN ('mandato', 'partido')),
    note                text,
    created_at          timestamptz NOT NULL DEFAULT now(),
    revoked_at          timestamptz,
    CHECK (citizen_id <> attester_citizen_id),
    UNIQUE (citizen_id, attester_citizen_id)
);

CREATE INDEX citizen_attestation_citizen_idx
    ON citizen_attestation (citizen_id)
    WHERE revoked_at IS NULL;

COMMENT ON TABLE citizen_attestation IS
    '0.28.3: web-of-trust — operador verificado atesta cidadão; selo público revogável.';

ALTER TABLE citizen_attestation OWNER TO dsoc;

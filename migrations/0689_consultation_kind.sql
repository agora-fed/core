-- Add a `kind` discriminator to consultations so the platform distinguishes
-- between open federated consultations ("consulta") and SSO-gated plebiscites
-- ("plebiscito"). Existing rows default to "plebiscito" per operator request.

ALTER TABLE consultations_consultation
    ADD COLUMN kind text NOT NULL DEFAULT 'plebiscito'
    CHECK (kind IN ('consulta', 'plebiscito'));

COMMENT ON COLUMN consultations_consultation.kind IS
    'consulta = open federated poll (ActivityPub Question), plebiscito = SSO-only agree/neutral/disagree';

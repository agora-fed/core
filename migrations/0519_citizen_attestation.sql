-- 0519_citizen_attestation.sql — citizenship attestation by a verified
-- operator (web-of-trust, 0.28.3). While institutional verification is
-- unavailable (TSE/gov.br denied on 2026-07-10), whoever is ALREADY
-- verified — a mandate operator (mandate_identity_binding) or an accepted
-- party admin (party_administrator) — can publicly attest that they know
-- the citizen. Auditable, revocable, badge on the public profile.
-- FK only to `citizen` (core identity table — REGISTRY rule).

CREATE TABLE citizen_attestation (
    id                  uuid PRIMARY KEY,
    citizen_id          uuid NOT NULL REFERENCES citizen(id),
    attester_citizen_id uuid NOT NULL REFERENCES citizen(id),
    -- The authority that legitimised the attestation AT THE TIME it was given.
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

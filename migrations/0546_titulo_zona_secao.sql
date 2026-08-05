-- Migration 0546 — electoral zone and section on the voter registration card.
--
-- Complements 0105: besides the card number, a citizen may record the ZONE
-- and SECTION where they vote (both printed on the card/e-Título). These are
-- auxiliary data — they do not take part in the check-digit validation and do
-- not change `titulo_status`; they serve future fine-grained territorial
-- segmentation (polling place) and a TSE cross-check once available.
--
-- TSE format: zone up to 4 digits, section up to 4 digits. Stored as
-- normalised text (digits only).
--
-- Idempotent: rerun-safe.

BEGIN;

ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS titulo_zona text
        CHECK (titulo_zona IS NULL OR titulo_zona ~ '^[0-9]{1,4}$'),
    ADD COLUMN IF NOT EXISTS titulo_secao text
        CHECK (titulo_secao IS NULL OR titulo_secao ~ '^[0-9]{1,4}$');

COMMENT ON COLUMN citizen.titulo_zona IS
    '0546: zona eleitoral (até 4 dígitos) declarada pelo cidadão — auxiliar, não valida o título.';
COMMENT ON COLUMN citizen.titulo_secao IS
    '0546: seção eleitoral (até 4 dígitos) declarada pelo cidadão — auxiliar, não valida o título.';

COMMIT;

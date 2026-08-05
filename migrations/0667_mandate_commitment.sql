-- 0666_mandate_commitment.sql — Mandato coletivo: compromisso consultivo declarado (D8.1).
--
-- Tese (accountability ≠ poder): um mandato coletivo (Bancada Ativista/SP, Gabinetona) se
-- PUBLICLY COMMITS to listening to its base before voting on a topic. The commitment is
-- VOLUNTARY and CONSULTATIVE — a mandate is non-delegable by law, so the software delivers only
-- the TRANSPARENCY of the commitment, NEVER legal binding. The UI never says "binding".
--
-- Flow: (1) the operator declares a commitment (topic + description); (2) optionally opens a
-- CONSULTATION with the base (reusing `consultations_consultation` + `_question`, ADR-0014 — answers/
-- aggregation come for free from the /consulta page); (3) publicly records whether it FOLLOWED the
-- result or not. The scorecard stops being "accusation" and becomes "instruction".
--
-- FKs: `mandate_id` → tabela de identidade central (permitida por REGISTRY.md). `consultation_id`
-- → `consultations_consultation` (the consultations crate) — a legitimate intra-platform FK, declared
-- em scripts/fk-allow.txt (mesmo caso do broadcast→consulta, 0656).
--
-- Idempotente: rerun-safe.

BEGIN;

CREATE TABLE IF NOT EXISTS mandate_commitment (
    id              uuid PRIMARY KEY,
    mandate_id      uuid NOT NULL REFERENCES mandate(id),
    theme           text NOT NULL,
    description     text NOT NULL,
    -- Only the consultative type exists. The CHECK locks any promise of legal binding in the
    -- schema itself — not even a code bug can store a "binding" commitment.
    kind            text NOT NULL DEFAULT 'consultivo' CHECK (kind = 'consultivo'),
    -- The consultation linked to the base (reusing the consultations crate). NULL until one is opened.
    consultation_id uuid REFERENCES consultations_consultation(id),
    -- The commitment's declared result: the mandate FOLLOWED the base, did NOT, or it is still pending.
    outcome         text CHECK (outcome IN ('seguiu', 'nao_seguiu', 'pendente')),
    outcome_note    text,
    -- Commitments are public by default (transparency is the product).
    is_public       boolean NOT NULL DEFAULT true,
    created_at      timestamptz NOT NULL DEFAULT now()
);

-- Public listing per mandate (the official's profile), most recent first.
CREATE INDEX IF NOT EXISTS mandate_commitment_mandate_idx
    ON mandate_commitment (mandate_id, created_at DESC);

COMMENT ON TABLE mandate_commitment IS
    '0666 (D8.1): compromisso consultivo VOLUNTÁRIO de um mandato coletivo — declara ouvir a base '
    'sobre um tema e publica se seguiu. Nunca vinculante (mandato é indelegável por lei).';
COMMENT ON COLUMN mandate_commitment.kind IS
    'Sempre ''consultivo'' (CHECK trava). Marca no schema que não há promessa de vinculação jurídica.';
COMMENT ON COLUMN mandate_commitment.outcome IS
    'seguiu | nao_seguiu | pendente — resultado público e imutável do compromisso.';

-- OWNER: o pod do gateway conecta como dsoc.
-- OWNER: ALTER TABLE mandate_commitment OWNER TO dsoc
ALTER TABLE mandate_commitment OWNER TO dsoc;

COMMIT;

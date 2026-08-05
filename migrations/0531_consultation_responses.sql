-- Migration 0531 — public consultation answers (0.44.0, Phase 3.3).
--
-- The consultations crate had consultations and questions, but NO answering
-- mechanism — the "public consultation" could not be answered. This table closes that:
-- each citizen gives ONE answer per question (agree/neutral/disagree),
-- editable (upsert). The public/participatory surface lives in
-- `crates/gateway/src/consultas_ext.rs` (runtime queries). FKs point at the questions
-- table (owner of the data) and `citizen` (central identity) — cf. REGISTRY.md.

BEGIN;

CREATE TABLE consultation_response (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    question_id  uuid NOT NULL
                 REFERENCES consultations_consultation_question(id) ON DELETE CASCADE,
    citizen_id   uuid NOT NULL REFERENCES citizen(id),
    answer       text NOT NULL CHECK (answer IN ('concordo', 'neutro', 'discordo')),
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    -- One answer per citizen per question (the upsert updates it).
    UNIQUE (question_id, citizen_id)
);

-- Per-question aggregation (count per option) without a full scan.
CREATE INDEX consultation_response_question_idx
    ON consultation_response (question_id, answer);
-- "My answers" in this consultation.
CREATE INDEX consultation_response_citizen_idx
    ON consultation_response (citizen_id);

COMMENT ON TABLE consultation_response IS
    '0.44.0: resposta de um cidadão a uma pergunta de consulta (concordo/neutro/discordo).';

-- The gateway pod connects as dsoc.
ALTER TABLE consultation_response OWNER TO dsoc;

COMMIT;

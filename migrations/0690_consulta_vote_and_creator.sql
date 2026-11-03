-- 0690: single-choice vote table for consulta kind + creator tracking.
--
-- Plebiscitos keep concordo/neutro/discordo via consultation_response.
-- Consultas (AP Question) use consultation_vote: one row per voter picking
-- one question (which acts as an option in the AP oneOf sense).
--
-- created_by tracks who opened the consultation so the AP object can
-- carry attributedTo pointing to a real Person actor.

ALTER TABLE consultations_consultation
    ADD COLUMN created_by uuid REFERENCES citizen(id);

CREATE TABLE consultation_vote (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    consultation_id uuid NOT NULL REFERENCES consultations_consultation(id) ON DELETE CASCADE,
    question_id  uuid NOT NULL REFERENCES consultations_consultation_question(id) ON DELETE CASCADE,
    citizen_id   uuid REFERENCES citizen(id),
    actor_url    text,
    created_at   timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT consultation_vote_exactly_one_identity
        CHECK (num_nonnulls(citizen_id, actor_url) = 1),
    CONSTRAINT consultation_vote_one_per_voter
        UNIQUE (consultation_id, citizen_id),
    CONSTRAINT consultation_vote_one_per_remote
        UNIQUE (consultation_id, actor_url)
);

CREATE INDEX idx_consultation_vote_question ON consultation_vote(question_id);

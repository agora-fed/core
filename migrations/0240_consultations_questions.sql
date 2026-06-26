-- 0240_consultations_questions — owned by `dsoc-consultations` (Tier 2). Scoped questions put
-- to the population with a defined response window. Explicit, auditable SQL (PLAN.md principle 3:
-- no ORM, no SELECT *). Cross-crate FKs target ONLY core identity tables (org) per
-- migrations/REGISTRY.md; the question FK targets a sibling table created in THIS migration.

-- ---------------------------------------------------------------------------
-- consultations_consultation — a consultation: a titled set of questions with a response
-- window [opens_at, closes_at). A consultation is a participation *space* instance, so its
-- primary key is a `spc_` SpaceId (uuid). `status` is a closed enum ('open'|'closed') guarded
-- by a CHECK; transitions are state-guarded in the service layer (open -> closed only).
-- ---------------------------------------------------------------------------
CREATE TABLE consultations_consultation (
    id         uuid PRIMARY KEY,
    org_id     uuid NOT NULL REFERENCES org(id),
    title      text NOT NULL,
    opens_at   timestamptz NOT NULL,
    closes_at  timestamptz NOT NULL,
    status     text NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'closed')),
    created_at timestamptz NOT NULL,
    -- A coherent window: the consultation opens strictly before it closes.
    CONSTRAINT consultations_consultation_window CHECK (opens_at < closes_at)
);

-- Keyset-friendly browse within an org, ordered by id (UUIDv7 ⇒ creation order).
CREATE INDEX consultations_consultation_org_idx
    ON consultations_consultation (org_id, id);

-- ---------------------------------------------------------------------------
-- consultations_consultation_question — an ordered question belonging to a consultation.
-- The FK targets the sibling consultation table (same migration, same crate — allowed by the
-- crate-isolation rule). `position` is 0-based and unique within a consultation.
-- ---------------------------------------------------------------------------
CREATE TABLE consultations_consultation_question (
    id              uuid PRIMARY KEY,
    consultation_id uuid NOT NULL
        REFERENCES consultations_consultation (id) ON DELETE CASCADE,
    prompt          text NOT NULL,
    position        integer NOT NULL CHECK (position >= 0),
    created_at      timestamptz NOT NULL,
    UNIQUE (consultation_id, position)
);

-- Fetch a consultation's questions in display order without a sort.
CREATE INDEX consultations_question_consultation_idx
    ON consultations_consultation_question (consultation_id, position);

COMMENT ON TABLE consultations_consultation IS
    'dsoc-consultations: a scoped set of questions with a defined response window.';
COMMENT ON TABLE consultations_consultation_question IS
    'dsoc-consultations: an ordered question belonging to a consultation.';

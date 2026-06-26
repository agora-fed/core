-- 0360_surveys_tables — owned by `dsoc-surveys` (range 0360, see migrations/REGISTRY.md).
-- Typed questionnaires: a survey holds typed questions and collects one response per
-- citizen. Explicit, auditable SQL (PLAN.md principle 3 — no ORM, sqlx compile-time-checked
-- queries). PostgreSQL 16+.
--
-- Cross-crate foreign keys point ONLY at the core identity tables `org` and `citizen`
-- (migrations/REGISTRY.md / ARCHITECTURE.md section 3). `survey_question.survey_id` and
-- `survey_response.survey_id` are INTRA-crate FKs to this crate's own `survey` table,
-- which is allowed.

-- ---------------------------------------------------------------------------
-- survey — a typed questionnaire owned by an org. `published_at` is NULL while
-- the survey is a draft and is stamped (from the injected clock) when it is
-- published; only published surveys accept responses.
-- ---------------------------------------------------------------------------
CREATE TABLE survey (
    id            uuid PRIMARY KEY,
    org_id        uuid NOT NULL REFERENCES org(id),
    title         text NOT NULL CHECK (length(btrim(title)) > 0),
    published_at  timestamptz,
    created_at    timestamptz NOT NULL
);
-- Keyset pagination over ascending id (UUIDv7 = time-ordered), scoped by org.
CREATE INDEX survey_org_idx ON survey (org_id, id);

-- ---------------------------------------------------------------------------
-- survey_question — one typed question on a survey. `kind` is constrained to the
-- closed set the domain enum enforces; `position` orders the questions on the
-- form. `survey_id` is an intra-crate FK.
-- ---------------------------------------------------------------------------
CREATE TABLE survey_question (
    id          uuid PRIMARY KEY,
    survey_id   uuid NOT NULL REFERENCES survey(id),
    prompt      text NOT NULL CHECK (length(btrim(prompt)) > 0),
    kind        text NOT NULL
                CHECK (kind IN ('text', 'single_choice', 'multiple_choice', 'scale', 'boolean')),
    position    integer NOT NULL CHECK (position >= 0),
    created_at  timestamptz NOT NULL
);
-- Keyset pagination over ascending id within a survey.
CREATE INDEX survey_question_survey_idx ON survey_question (survey_id, id);

-- ---------------------------------------------------------------------------
-- survey_response — one citizen's answers to a survey, stored as a `jsonb`
-- object keyed by question. `citizen_id` is a cross-crate FK to the core
-- identity table `citizen` (allowed); `survey_id` is an intra-crate FK. The
-- UNIQUE (survey_id, citizen_id) enforces the one-response-per-citizen rule.
-- ---------------------------------------------------------------------------
CREATE TABLE survey_response (
    id          uuid PRIMARY KEY,
    survey_id   uuid NOT NULL REFERENCES survey(id),
    citizen_id  uuid NOT NULL REFERENCES citizen(id),
    answers     jsonb NOT NULL,
    created_at  timestamptz NOT NULL,
    UNIQUE (survey_id, citizen_id)
);
-- Keyset pagination over ascending id within a survey (oldest-first).
CREATE INDEX survey_response_survey_idx ON survey_response (survey_id, id);

COMMENT ON TABLE survey IS 'Typed questionnaire (draft until published_at is set); owned by dsoc-surveys.';
COMMENT ON TABLE survey_question IS 'One typed question on a survey; owned by dsoc-surveys.';
COMMENT ON TABLE survey_response IS 'One citizen response (answers jsonb), one per citizen; owned by dsoc-surveys.';

-- Migration 0532 — targeted polls of the campaign group (0.45.0, Phase 3.4).
--
-- The campaign group (0527) was broadcast only: the politician PUBLISHES and the voter READS.
-- The two-way leg was missing — the politician asking their base and hearing the answer. Here
-- they open a "quick poll" (one question, agree/neutral/disagree) targeted at their
-- group; the logged-in citizen answers and the result aggregates live. Same aggregation
-- engine as the consultations (0531), but with an OWNER (the group's mandate) —
-- it is the proactive campaign→voter channel the plan calls for in 3.4.

BEGIN;

CREATE TABLE campaign_group_poll (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    group_id    uuid NOT NULL REFERENCES campaign_group(id) ON DELETE CASCADE,
    question    text NOT NULL,
    status      text NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'closed')),
    created_at  timestamptz NOT NULL DEFAULT now(),
    closed_at   timestamptz
);
CREATE INDEX campaign_group_poll_group_idx
    ON campaign_group_poll (group_id, created_at DESC, id DESC);

CREATE TABLE campaign_group_poll_response (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    poll_id     uuid NOT NULL REFERENCES campaign_group_poll(id) ON DELETE CASCADE,
    citizen_id  uuid NOT NULL REFERENCES citizen(id),
    answer      text NOT NULL CHECK (answer IN ('concordo', 'neutro', 'discordo')),
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    -- One answer per citizen per poll (the upsert updates it).
    UNIQUE (poll_id, citizen_id)
);
CREATE INDEX campaign_group_poll_response_poll_idx
    ON campaign_group_poll_response (poll_id, answer);

COMMENT ON TABLE campaign_group_poll IS
    '0.45.0: enquete rápida dirigida pelo dono do grupo de campanha à sua base.';
COMMENT ON TABLE campaign_group_poll_response IS
    '0.45.0: resposta de um cidadão a uma enquete de campanha (concordo/neutro/discordo).';

-- The gateway pod connects as dsoc.
ALTER TABLE campaign_group_poll OWNER TO dsoc;
ALTER TABLE campaign_group_poll_response OWNER TO dsoc;

COMMIT;

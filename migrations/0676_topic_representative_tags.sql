-- 0676 — tag-a-representative (issue agora-fed/core#3).
--
-- A citizen marks ONE mandate (deputy/representative) per forum topic as the
-- person who should represent them on that cause. Once a day the worker
-- compiles every mandate's new tags and sends ONE consolidated alert e-mail
-- to the mandate's public address (until the official onboards and switches
-- to in-platform notifications).
--
-- Privacy (ADR-0005 posture): public reads expose AGGREGATES per mandate
-- only; the citizen linkage stays in this table and is never listed.

CREATE TABLE topic_representative_tag (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id      uuid NOT NULL REFERENCES org(id),
    topic_id    uuid NOT NULL REFERENCES forum_topic(id) ON DELETE CASCADE,
    mandate_id  uuid NOT NULL REFERENCES mandate(id) ON DELETE CASCADE,
    citizen_id  uuid NOT NULL REFERENCES citizen(id),
    created_at  timestamptz NOT NULL DEFAULT now(),
    -- One representative per citizen per topic (re-tagging replaces).
    UNIQUE (org_id, topic_id, citizen_id)
);

CREATE INDEX topic_representative_tag_mandate_day_idx
    ON topic_representative_tag (mandate_id, created_at);
CREATE INDEX topic_representative_tag_topic_idx
    ON topic_representative_tag (topic_id);

COMMENT ON TABLE topic_representative_tag IS
    'gateway: citizen marks a mandate to represent them on a forum topic (issue #3). Public reads are aggregate-only.';

-- Daily consolidated alert to the mandate: one row per (mandate, day) claims
-- the send — the sweep is idempotent by primary key.
CREATE TABLE mandate_alert_delivery (
    mandate_id  uuid NOT NULL REFERENCES mandate(id) ON DELETE CASCADE,
    day         date NOT NULL,
    tag_count   integer NOT NULL,
    sent_at     timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (mandate_id, day)
);

COMMENT ON TABLE mandate_alert_delivery IS
    'gateway: idempotency claim + receipt of the daily consolidated representative-tag e-mail per mandate.';

-- Production applies migrations as postgres while the gateway runs as dsoc
-- (same as 0675) — align ownership where that role exists.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'dsoc') THEN
        ALTER TABLE topic_representative_tag OWNER TO dsoc;
        ALTER TABLE mandate_alert_delivery OWNER TO dsoc;
    END IF;
END $$;

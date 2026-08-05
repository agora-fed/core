-- Migration 0527 — campaign groups (Phase 2.3).
--
-- A "campaign group" is the proactive campaign→voter channel that was missing: today
-- the official only REACTS to demands under an SLA (the mandate panel). Here they CREATE
-- their own space — the voter joins, the campaign publishes updates, and
-- the supporter base becomes visible and mobilizable.
--
-- Owner = a mandate (the same binding as the is_politico gate / panel / campaign).
-- It does not reuse `assembly` (0220): that is a participatory body of the org, with NO owner —
-- different semantics. Dedicated, lean tables.

BEGIN;

-- campaign_group — one group per mandate (UNIQUE mandate_id). owner_citizen_id
-- records who created it (the official logged in at the time).
CREATE TABLE campaign_group (
    id                uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id            uuid NOT NULL REFERENCES org(id),
    mandate_id        uuid NOT NULL REFERENCES mandate(id),
    owner_citizen_id  uuid NOT NULL REFERENCES citizen(id),
    name              text NOT NULL,
    description       text,
    created_at        timestamptz NOT NULL DEFAULT now(),
    -- One group per mandate: the official has ONE campaign space.
    UNIQUE (mandate_id)
);
CREATE INDEX campaign_group_org_idx ON campaign_group (org_id, id);

-- campaign_group_member — the supporter roster. UNIQUE makes the join idempotent.
CREATE TABLE campaign_group_member (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    group_id    uuid NOT NULL REFERENCES campaign_group(id) ON DELETE CASCADE,
    citizen_id  uuid NOT NULL REFERENCES citizen(id),
    joined_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (group_id, citizen_id)
);
CREATE INDEX campaign_group_member_group_idx ON campaign_group_member (group_id, id);
CREATE INDEX campaign_group_member_citizen_idx ON campaign_group_member (citizen_id);

-- campaign_group_post — updates published by the campaign (only the owner posts in the MVP).
CREATE TABLE campaign_group_post (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    group_id    uuid NOT NULL REFERENCES campaign_group(id) ON DELETE CASCADE,
    body        text NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX campaign_group_post_group_idx ON campaign_group_post (group_id, created_at DESC, id DESC);

COMMENT ON TABLE campaign_group IS
    '0.39.0: espaço de campanha de um mandato — canal proativo campanha→eleitor.';
COMMENT ON TABLE campaign_group_member IS
    '0.39.0: apoiadores que entraram no grupo (join idempotente).';
COMMENT ON TABLE campaign_group_post IS
    '0.39.0: atualizações publicadas pela campanha no grupo.';

-- The gateway pod connects as `dsoc` (not postgres) — without this, permission denied.
ALTER TABLE campaign_group OWNER TO dsoc;
ALTER TABLE campaign_group_member OWNER TO dsoc;
ALTER TABLE campaign_group_post OWNER TO dsoc;

COMMIT;

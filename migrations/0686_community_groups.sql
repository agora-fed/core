-- Working groups of a community. Separate from campaign_group, which serves candidacies.

CREATE TABLE community_group (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES org (id),
    slug        text NOT NULL CHECK (slug ~ '^[a-z0-9][a-z0-9-]{2,60}$'),
    name        text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 120),
    description text NOT NULL DEFAULT '',
    entry       text NOT NULL DEFAULT 'approval' CHECK (entry IN ('open', 'approval')),
    forum_id    uuid REFERENCES forum (id),
    created_by  uuid REFERENCES citizen (id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    archived_at timestamptz,
    UNIQUE (org_id, slug)
);

CREATE TABLE community_group_member (
    group_id   uuid NOT NULL REFERENCES community_group (id) ON DELETE CASCADE,
    citizen_id uuid NOT NULL REFERENCES citizen (id) ON DELETE CASCADE,
    role       text NOT NULL DEFAULT 'member' CHECK (role IN ('coordinator', 'member')),
    status     text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'pending')),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (group_id, citizen_id)
);
CREATE INDEX community_group_member_citizen_idx ON community_group_member (citizen_id);

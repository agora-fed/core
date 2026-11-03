-- Restricted forums. `forum_readable` is the single access rule: every code path that
-- shows forum content goes through it, and anything it does not recognize is denied.

ALTER TABLE forum
    ADD COLUMN visibility text NOT NULL DEFAULT 'public'
        CONSTRAINT forum_visibility_check CHECK (visibility IN ('public', 'members', 'list')),
    ADD CONSTRAINT forum_restricted_not_federated CHECK (visibility = 'public' OR NOT federated);

CREATE TABLE forum_member (
    forum_id   uuid NOT NULL REFERENCES forum (id) ON DELETE CASCADE,
    citizen_id uuid NOT NULL REFERENCES citizen (id) ON DELETE CASCADE,
    added_by   uuid REFERENCES citizen (id) ON DELETE SET NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (forum_id, citizen_id)
);
CREATE INDEX forum_member_citizen_idx ON forum_member (citizen_id);

-- A forum is readable when it and all its ancestors are. A NULL citizen is an anonymous
-- visitor or a remote server, which can only ever read public forums.
CREATE FUNCTION forum_readable(p_forum uuid, p_citizen uuid) RETURNS boolean
LANGUAGE sql STABLE AS $$
    WITH RECURSIVE chain AS (
        SELECT id, parent_id, org_id, visibility, hidden_at FROM forum WHERE id = p_forum
        UNION ALL
        SELECT f.id, f.parent_id, f.org_id, f.visibility, f.hidden_at
          FROM forum f JOIN chain c ON f.id = c.parent_id
    )
    SELECT EXISTS (SELECT 1 FROM chain) AND NOT EXISTS (
        SELECT 1 FROM chain n
         WHERE n.hidden_at IS NOT NULL
            OR (n.visibility <> 'public' AND NOT (
                    EXISTS (
                        SELECT 1 FROM citizen v
                         WHERE v.id = p_citizen AND v.org_id = n.org_id
                           AND v.deleted_at IS NULL AND v.suspended_at IS NULL
                    )
                AND (
                        n.visibility = 'members'
                     OR (n.visibility = 'list' AND (
                            EXISTS (SELECT 1 FROM forum_member m
                                     WHERE m.forum_id = n.id AND m.citizen_id = p_citizen)
                         OR EXISTS (SELECT 1 FROM forum_moderator m
                                     WHERE m.forum_id = n.id AND m.citizen_id = p_citizen)
                        ))
                    )
            ))
    )
$$;

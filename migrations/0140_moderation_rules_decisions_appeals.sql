-- 0140_moderation_rules_decisions_appeals — owned by `dsoc-moderation`.
-- Auditable moderation: deterministic rules + a transparent statistical signal
-- (PLAN.md correction #3, principle 11 — no opaque third-party classifier).
-- Explicit SQL, compile-time-checked by sqlx. PostgreSQL 16+.
--
-- Cross-crate foreign keys point ONLY at the core identity table `org`
-- (ARCHITECTURE.md section 3 / migrations/REGISTRY.md). Proposal/comment targets
-- are stored as bare uuids: moderation never reaches into another crate's tables.

-- ---------------------------------------------------------------------------
-- moderation_rule — the deterministic, human-readable ruleset per organization.
-- `kind` selects the matcher; `pattern` is its (auditable) parameter; `action`
-- records the prescribed response (soft flag vs hard reject).
-- ---------------------------------------------------------------------------
CREATE TABLE moderation_rule (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES org(id),
    kind        text NOT NULL CHECK (kind IN ('keyword', 'caps_ratio')),
    pattern     text NOT NULL CHECK (length(pattern) > 0),
    action      text NOT NULL CHECK (action IN ('flag', 'reject')),
    created_at  timestamptz NOT NULL
);

-- Evaluation fetches an org's rules oldest-first (deterministic precedence);
-- this index also backs keyset listing.
CREATE INDEX moderation_rule_org_idx
    ON moderation_rule (org_id, created_at, id);

-- ---------------------------------------------------------------------------
-- moderation_decision — the audit record. EVERY evaluation writes exactly one
-- row, whether flagged or cleared: decisions are never silently dropped.
-- `rule_id` is the rule that matched (NULL when content cleared).
-- ---------------------------------------------------------------------------
CREATE TABLE moderation_decision (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES org(id),
    target_kind text NOT NULL CHECK (target_kind IN ('proposal', 'comment')),
    target_id   uuid NOT NULL,
    rule_id     uuid REFERENCES moderation_rule(id),
    outcome     text NOT NULL CHECK (outcome IN ('flagged', 'cleared')),
    created_at  timestamptz NOT NULL,
    -- A cleared decision has no matching rule; a flagged decision must name one.
    CONSTRAINT moderation_decision_rule_consistency CHECK (
        (outcome = 'cleared' AND rule_id IS NULL)
        OR (outcome = 'flagged' AND rule_id IS NOT NULL)
    )
);

-- Keyset pagination of the audit log: newest-first by (created_at, id).
CREATE INDEX moderation_decision_org_idx
    ON moderation_decision (org_id, created_at DESC, id DESC);
-- Look up the moderation history of a specific target.
CREATE INDEX moderation_decision_target_idx
    ON moderation_decision (target_kind, target_id);

-- ---------------------------------------------------------------------------
-- moderation_appeal — a citizen's challenge to a decision. A small state
-- machine: open -> granted | denied. Transitions are auditable via updated_at.
-- ---------------------------------------------------------------------------
CREATE TABLE moderation_appeal (
    id          uuid PRIMARY KEY,
    decision_id uuid NOT NULL REFERENCES moderation_decision(id),
    reason      text NOT NULL CHECK (length(reason) > 0),
    status      text NOT NULL CHECK (status IN ('open', 'granted', 'denied')),
    created_at  timestamptz NOT NULL,
    updated_at  timestamptz NOT NULL
);

CREATE INDEX moderation_appeal_decision_idx
    ON moderation_appeal (decision_id, created_at DESC, id DESC);

COMMENT ON TABLE moderation_rule IS 'Auditable moderation ruleset; owned by dsoc-moderation.';
COMMENT ON TABLE moderation_decision IS 'Append-only moderation audit log; one row per evaluation.';
COMMENT ON TABLE moderation_appeal IS 'Citizen appeals against moderation decisions (open->granted|denied).';

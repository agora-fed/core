-- 0330_debates_tables — owned by `dsoc-debates` (range 0330, see migrations/REGISTRY.md).
-- Structured debates: framed pro/con deliberation spaces. Explicit, auditable SQL
-- (PLAN.md principle 3 — no ORM, sqlx compile-time-checked queries). PostgreSQL 16+.
--
-- Cross-crate foreign keys point ONLY at the core identity tables `org` and `citizen`
-- (migrations/REGISTRY.md / ARCHITECTURE.md section 3). `debate_contribution.debate_id`
-- is an INTRA-crate FK to this crate's own `debate` table, which is allowed.

-- ---------------------------------------------------------------------------
-- debate — a framed deliberation space. `title` is the question/motion under
-- debate; `framing` is the neutral context that frames the pro/con discussion.
-- ---------------------------------------------------------------------------
CREATE TABLE debate (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES org(id),
    title       text NOT NULL CHECK (length(btrim(title)) > 0),
    framing     text NOT NULL CHECK (length(btrim(framing)) > 0),
    created_at  timestamptz NOT NULL
);
-- Keyset pagination over ascending id (UUIDv7 = time-ordered), scoped by org.
CREATE INDEX debate_org_idx ON debate (org_id, id);

-- ---------------------------------------------------------------------------
-- debate_contribution — one citizen's contribution to a debate, taking a
-- `stance` of pro/con/neutral. `author_id` is a cross-crate FK to the core
-- identity table `citizen` (allowed); `debate_id` is an intra-crate FK.
-- ---------------------------------------------------------------------------
CREATE TABLE debate_contribution (
    id          uuid PRIMARY KEY,
    debate_id   uuid NOT NULL REFERENCES debate(id),
    author_id   uuid NOT NULL REFERENCES citizen(id),
    stance      text NOT NULL CHECK (stance IN ('pro', 'con', 'neutral')),
    body        text NOT NULL CHECK (length(btrim(body)) > 0),
    created_at  timestamptz NOT NULL
);
-- Keyset pagination over ascending id within a debate (oldest-first).
CREATE INDEX debate_contribution_debate_idx ON debate_contribution (debate_id, id);

COMMENT ON TABLE debate IS 'Framed pro/con deliberation space; owned by dsoc-debates.';
COMMENT ON TABLE debate_contribution IS 'One citizen contribution (pro/con/neutral) to a debate; owned by dsoc-debates.';

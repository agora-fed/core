-- 0350_budgets_tables — owned by `dsoc-budgets` (range 0350, see migrations/REGISTRY.md).
-- Participatory budgeting: projects, costs, and citizen allocation under a ceiling.
-- Explicit, auditable SQL (PLAN.md principle 3 — no ORM, sqlx compile-time-checked
-- queries). PostgreSQL 16+.
--
-- Cross-crate foreign keys point ONLY at the core identity tables `org` and `citizen`
-- (migrations/REGISTRY.md / ARCHITECTURE.md section 3). `budget_project.budget_id`,
-- `budget_order.budget_id`, and the `budget_order_item` links are INTRA-crate FKs to
-- this crate's own tables, which is allowed.

-- ---------------------------------------------------------------------------
-- budget — a participatory budgeting round under a fixed spending `ceiling_cents`.
-- Citizens allocate to projects (below) and the sum of an order must never exceed
-- this ceiling (enforced in the domain, see crates/components/budgets/src/domain.rs).
-- ---------------------------------------------------------------------------
CREATE TABLE budget (
    id            uuid PRIMARY KEY,
    org_id        uuid NOT NULL REFERENCES org(id),
    title         text NOT NULL CHECK (length(btrim(title)) > 0),
    ceiling_cents bigint NOT NULL CHECK (ceiling_cents > 0),
    created_at    timestamptz NOT NULL
);
-- Keyset pagination over ascending id (UUIDv7 = time-ordered), scoped by org.
CREATE INDEX budget_org_idx ON budget (org_id, id);

-- ---------------------------------------------------------------------------
-- budget_project — a fundable project within a budget, with a positive `cost_cents`.
-- `budget_id` is an intra-crate FK to this crate's own `budget` table (allowed).
-- ---------------------------------------------------------------------------
CREATE TABLE budget_project (
    id          uuid PRIMARY KEY,
    budget_id   uuid NOT NULL REFERENCES budget(id),
    title       text NOT NULL CHECK (length(btrim(title)) > 0),
    cost_cents  bigint NOT NULL CHECK (cost_cents > 0),
    created_at  timestamptz NOT NULL
);
-- Keyset pagination over ascending id within a budget.
CREATE INDEX budget_project_budget_idx ON budget_project (budget_id, id);

-- ---------------------------------------------------------------------------
-- budget_order — a citizen's allocation within a budget. The UNIQUE (budget_id,
-- citizen_id) makes ordering a guarded state transition: a citizen has at most one
-- order per budget (a duplicate maps to `Error::Conflict`). `citizen_id` is a
-- cross-crate FK to the core identity table `citizen` (allowed); `budget_id` is
-- an intra-crate FK.
-- ---------------------------------------------------------------------------
CREATE TABLE budget_order (
    id          uuid PRIMARY KEY,
    budget_id   uuid NOT NULL REFERENCES budget(id),
    citizen_id  uuid NOT NULL REFERENCES citizen(id),
    created_at  timestamptz NOT NULL,
    UNIQUE (budget_id, citizen_id)
);
-- Keyset pagination over ascending id within a budget.
CREATE INDEX budget_order_budget_idx ON budget_order (budget_id, id);

-- ---------------------------------------------------------------------------
-- budget_order_item — links an order to each project it allocates to. Both FKs are
-- INTRA-crate (to `budget_order` and `budget_project`); the composite PRIMARY KEY
-- makes a project appear at most once per order.
-- ---------------------------------------------------------------------------
CREATE TABLE budget_order_item (
    order_id    uuid NOT NULL REFERENCES budget_order(id),
    project_id  uuid NOT NULL REFERENCES budget_project(id),
    PRIMARY KEY (order_id, project_id)
);
-- Reverse lookup: which orders include a given project.
CREATE INDEX budget_order_item_project_idx ON budget_order_item (project_id);

COMMENT ON TABLE budget IS 'Participatory budgeting round under a spending ceiling; owned by dsoc-budgets.';
COMMENT ON TABLE budget_project IS 'A fundable project within a budget; owned by dsoc-budgets.';
COMMENT ON TABLE budget_order IS 'A citizen allocation within a budget (one per citizen); owned by dsoc-budgets.';
COMMENT ON TABLE budget_order_item IS 'Links a budget order to each allocated project; owned by dsoc-budgets.';

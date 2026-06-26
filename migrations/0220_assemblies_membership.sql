-- 0220_assemblies_membership — owned by `dsoc-assemblies` (Tier 2, migration range 0220).
-- Assemblies are permanent participatory bodies with membership. This migration creates the two
-- tables the crate owns: the body itself (`assembly`) and its roster (`assembly_member`).
-- Explicit, auditable SQL (PLAN.md principle 3): no ORM, no SELECT *, keyset-friendly indexes.
-- Cross-crate FKs target ONLY core identity tables (`org`, `citizen`) and a table created in THIS
-- same migration (`assembly`), per migrations/REGISTRY.md. No IPv4 anywhere (PLAN.md principle 4).

-- ---------------------------------------------------------------------------
-- assembly — a permanent participatory body, scoped to one organization. Its id doubles as the
-- logical `SpaceId` for the assembly's participation space (no separate spaces table to consult).
-- ---------------------------------------------------------------------------
CREATE TABLE assembly (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES org(id),
    -- Public display name of the body (e.g. 'Assembleia Municipal de Saúde').
    name        text NOT NULL,
    created_at  timestamptz NOT NULL
);
-- Keyset pagination support: list an organization's assemblies ordered by id.
CREATE INDEX assembly_org_idx ON assembly (org_id, id);

-- ---------------------------------------------------------------------------
-- assembly_member — the roster binding a citizen to an assembly with a role. A citizen may hold
-- at most one membership per assembly (UNIQUE), making "add member" idempotent: re-adding the
-- same citizen returns the existing row rather than creating a duplicate.
-- ---------------------------------------------------------------------------
CREATE TABLE assembly_member (
    id           uuid PRIMARY KEY,
    assembly_id  uuid NOT NULL REFERENCES assembly(id),
    citizen_id   uuid NOT NULL REFERENCES citizen(id),
    -- The member's role within the body. Constrained to the sanctioned set, kept in sync with
    -- `dsoc_assemblies::domain::ALLOWED_ROLES`.
    role         text NOT NULL
                 CHECK (role IN ('member', 'chair', 'secretary', 'observer')),
    joined_at    timestamptz NOT NULL,
    created_at   timestamptz NOT NULL,
    -- One membership per citizen per assembly (drives idempotent add + the keyset roster).
    UNIQUE (assembly_id, citizen_id)
);
-- Keyset pagination support: list a single assembly's roster ordered by member id.
CREATE INDEX assembly_member_assembly_idx ON assembly_member (assembly_id, id);

COMMENT ON TABLE assembly IS 'dsoc-assemblies: a permanent participatory body, scoped to one org.';
COMMENT ON TABLE assembly_member IS 'dsoc-assemblies: assembly roster (one membership per citizen per assembly).';

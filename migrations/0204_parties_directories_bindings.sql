-- 0204_parties_directories_bindings — formalize the "partido" concept as a first-class entity
-- with subnational directories and human administrators. Owned by `dsoc-mandates` (range
-- 0200-0209 per migrations/REGISTRY.md). Phase 2B (roadmap F2 — Organisations/Parties).
--
-- BEFORE this migration, "partido" was DERIVED from `mandate.party` (the sigla). That worked
-- for public listing but couldn't model:
--   1. subnational directorates (national / state / municipal chapters),
--   2. platform administrators authorized to act on behalf of the party or a directory,
--   3. catalog fields absent from a mandate row (TSE number, founding year, logo).
--
-- Cross-crate FKs stay within the allowed set: `org(id)`, `citizen(id)`, and the new tables'
-- own ids. The `mandate.party` column is left untouched (existing consumers, incl. `web/src/
-- lib/parties.ts`, keep working exactly as-is). This migration is ADDITIVE only.
--
-- Multi-tenancy: every row carries `org_id`. The `party` PK is composite `(org_id, sigla)` so
-- two orgs can independently maintain a "PT" without collision. Sigla case is PRESERVED
-- ("PCdoB", not "pcdob") to mirror the mandate table and the TSE.

-- ---------------------------------------------------------------------------
-- party — canonical catalog of a political party inside an org.
-- ---------------------------------------------------------------------------
CREATE TABLE party (
    org_id       uuid NOT NULL REFERENCES org(id),
    -- Sigla (case-preserved). Composite PK with org_id so two tenants can maintain
    -- independent catalogs. Non-empty enforced by a CHECK to guard against blank rows
    -- (a legacy mandate with `party = ''` would otherwise slip in via the backfill).
    sigla        text NOT NULL CHECK (length(sigla) > 0),
    -- Human-readable name. Placeholder = sigla until an admin fills it in.
    name         text NOT NULL,
    founded_year integer,
    -- TSE numeric identifier (e.g. 13 = PT, 65 = PCdoB). NULL when unknown or unofficial.
    tse_number   integer,
    -- Opaque object-storage key or absolute URL for the crest/logo.
    logo_url     text,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, sigla)
);

COMMENT ON TABLE party IS
    'dsoc-mandates: canonical party catalog. Composite PK (org_id, sigla). Sigla case-preserved.';
COMMENT ON COLUMN party.sigla IS
    'Sigla como aparece na base oficial (case preservado: PT, PCdoB, PSOL...).';
COMMENT ON COLUMN party.tse_number IS
    'Número TSE do partido (13, 65, 50...); NULL quando desconhecido / não oficial.';

-- ---------------------------------------------------------------------------
-- party_directory — subnational directorate of a party (nacional, estadual, municipal).
-- The tree is expressed via `parent_directory_id` (a municipal directory typically points at
-- its estadual parent). The federative CHECK guarantees uf/municipio are consistent with the
-- declared esfera — the same three-value enum already used on `mandate` (migration 0203).
-- ---------------------------------------------------------------------------
CREATE TABLE party_directory (
    id                  uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id              uuid NOT NULL REFERENCES org(id),
    party_sigla         text NOT NULL,
    esfera              text NOT NULL
                        CHECK (esfera IN ('federal', 'estadual', 'municipal')),
    -- UF sigla (2 chars). Required for estadual and municipal, forbidden for federal.
    uf                  char(2),
    -- Municipality name. Required only for municipal.
    municipio           text,
    -- Chapter name (e.g. "Diretório Estadual do PT — Bahia"). Free-form.
    name                text NOT NULL,
    -- Auto-reference to the parent directory in the tree (a municipal usually points at its
    -- estadual, an estadual at the federal). NULL = top of tree.
    parent_directory_id uuid REFERENCES party_directory(id),
    created_at          timestamptz NOT NULL DEFAULT now(),
    -- Composite FK back to the party catalog so a directory can only exist for a real party.
    FOREIGN KEY (org_id, party_sigla) REFERENCES party(org_id, sigla),
    -- Federative consistency:
    --   federal     ⇒ uf IS NULL AND municipio IS NULL
    --   estadual    ⇒ uf IS NOT NULL AND municipio IS NULL
    --   municipal   ⇒ uf IS NOT NULL AND municipio IS NOT NULL
    CONSTRAINT party_directory_esfera_shape CHECK (
        (esfera = 'federal'   AND uf IS NULL     AND municipio IS NULL) OR
        (esfera = 'estadual'  AND uf IS NOT NULL AND municipio IS NULL) OR
        (esfera = 'municipal' AND uf IS NOT NULL AND municipio IS NOT NULL)
    )
);

CREATE INDEX party_directory_org_sigla_idx
    ON party_directory (org_id, party_sigla);
CREATE INDEX party_directory_parent_idx
    ON party_directory (parent_directory_id)
 WHERE parent_directory_id IS NOT NULL;

COMMENT ON TABLE party_directory IS
    'dsoc-mandates: diretório subnacional de um partido. Árvore via parent_directory_id.';

-- ---------------------------------------------------------------------------
-- party_administrator — a citizen authorized to administer a party (or a specific directory).
-- `directory_id IS NULL` means administrator of the party at the national level.
-- Invitation flow: `invited_by` + `accepted_at` mirror the existing mandate invitation shape.
-- ---------------------------------------------------------------------------
CREATE TABLE party_administrator (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id       uuid NOT NULL REFERENCES org(id),
    party_sigla  text NOT NULL,
    -- NULL = administrator of the party as a whole (national chapter / top level).
    directory_id uuid REFERENCES party_directory(id),
    citizen_id   uuid NOT NULL REFERENCES citizen(id),
    role         text NOT NULL CHECK (role IN ('admin', 'moderador')),
    invited_by   uuid REFERENCES citizen(id),
    accepted_at  timestamptz,
    created_at   timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (org_id, party_sigla) REFERENCES party(org_id, sigla),
    -- A citizen can hold at most one role per (party, directory) scope. `directory_id` is
    -- NULLABLE so this UNIQUE treats the NULL-slot (national admin) as its own bucket per
    -- Postgres semantics — that is EXACTLY what we want: many citizens can each be a national
    -- admin, but a given citizen can only appear ONCE with directory_id = NULL. The pair with
    -- a non-null directory_id also behaves distinctly per (party, directory).
    UNIQUE (org_id, party_sigla, directory_id, citizen_id)
);

CREATE INDEX party_administrator_org_sigla_idx
    ON party_administrator (org_id, party_sigla);
CREATE INDEX party_administrator_citizen_idx
    ON party_administrator (citizen_id);

COMMENT ON TABLE party_administrator IS
    'dsoc-mandates: cidadão autorizado a administrar o partido (directory_id NULL = nacional).';

-- ---------------------------------------------------------------------------
-- Backfill: seed `party` from the DISTINCT (org_id, party) pairs already present on `mandate`.
-- `name = sigla` is a placeholder (the UI already renders the sigla when the full name is
-- absent). Ignores blank / NULL rows to avoid failing the sigla non-empty CHECK.
-- ---------------------------------------------------------------------------
INSERT INTO party (org_id, sigla, name)
SELECT DISTINCT m.org_id, m.party, m.party
FROM mandate m
WHERE m.party IS NOT NULL AND m.party <> ''
ON CONFLICT (org_id, sigla) DO NOTHING;

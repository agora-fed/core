-- 0203_mandate_sphere.sql — federative sphere on the mandate registry.
--
-- Politicians in Brazil operate on three distinct governance spheres, and citizens
-- reason about them differently: FEDERAL (Câmara, Senado, Presidência), ESTADUAL
-- (Assembleia, governadoria), MUNICIPAL (Câmara municipal, prefeitura). The /politicos
-- UI needs to sub-filter by sphere. Seed today only carries the 107 federal Congress
-- members, so most rows land as `federal`; the other two sphères are ready to be
-- populated in future seeds (state assemblies, city councils) without a schema change.
--
-- Derivation rule (idempotent, best-effort): `house IN ('camara','senado')` implies
-- federal; anything mentioning "estadual" / "assembleia" / "governador" in office maps
-- to estadual; "municipal" / "vereador" / "prefeito" maps to municipal. Everything
-- else defaults to `federal` (safe for the current 107-row baseline).

ALTER TABLE mandate
    ADD COLUMN sphere text NOT NULL DEFAULT 'federal'
        CHECK (sphere IN ('federal', 'estadual', 'municipal'));

-- Backfill from office / house heuristics. Runs once at migration time; new inserts
-- should set sphere explicitly (the service layer does).
UPDATE mandate SET sphere = CASE
    WHEN house IN ('camara', 'senado') THEN 'federal'
    WHEN office ~* '(estadual|assembl[eé]ia|governador)' THEN 'estadual'
    WHEN office ~* '(municipal|vereador|prefeito)' THEN 'municipal'
    ELSE 'federal'
END;

-- Filter-by-sphere is the primary query the UI issues; the composite index covers the
-- (org, sphere) predicate plus the display_name ORDER BY the list already uses.
CREATE INDEX mandate_org_sphere_name_idx
    ON mandate (org_id, sphere, display_name);

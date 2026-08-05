-- 0673_party_directory_territorial_unique — one directory per party per territory.
--
-- MOTIVATION (incident 2026-07-31): two identical "Diretório Municipal PT-Ubatuba"
-- created 49s apart in production. 0204 validates the federative SHAPE (sphere ↔
-- uf/municipio) but nothing prevented two directories of the same party in the SAME territory —
-- qualquer duplo clique, retry de rede ou corrida entre abas criava outro.
--
-- NULLS NOT DISTINCT (PG15+) is essential: federal has uf/municipio NULL and state has
-- municipio NULL; with the default (NULLS DISTINCT) those spheres would stay duplicable.
-- Result: at most 1 federal, 1 state per UF and 1 municipal per (UF, municipality)
-- per (org, party). The name stays free — the territory is the identity, not the label.
--
-- The creation handlers (dsoc-mandates::parties and gateway::admin_parties) translate the
-- violation into a 409 `directory_exists`. Prerequisite: production already deduplicated (the Ubatuba
-- duplicate was removed manually before this migration — see CHANGELOG).
CREATE UNIQUE INDEX party_directory_territorio_key
    ON party_directory (org_id, party_sigla, esfera, uf, municipio)
    NULLS NOT DISTINCT;

COMMENT ON INDEX party_directory_territorio_key IS
    'dsoc-mandates: 1 diretório por (org, partido, esfera, uf, municipio); NULLS NOT DISTINCT cobre federal/estadual.';

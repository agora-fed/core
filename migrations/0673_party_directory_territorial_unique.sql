-- 0673_party_directory_territorial_unique — um diretório por partido por território.
--
-- MOTIVAÇÃO (incidente 2026-07-31): dois "Diretório Municipal PT-Ubatuba" idênticos
-- criados com 49s de intervalo em prod. A 0204 valida a FORMA federativa (esfera ↔
-- uf/municipio) mas nada impedia dois diretórios do mesmo partido no MESMO território —
-- qualquer duplo clique, retry de rede ou corrida entre abas criava outro.
--
-- NULLS NOT DISTINCT (PG15+) é essencial: federal tem uf/municipio NULL e estadual tem
-- municipio NULL; com o default (NULLS DISTINCT) essas esferas continuariam duplicáveis.
-- Resultado: no máximo 1 federal, 1 estadual por UF e 1 municipal por (UF, município)
-- por (org, partido). O nome segue livre — território é a identidade, não o rótulo.
--
-- Os handlers de criação (dsoc-mandates::parties e gateway::admin_parties) traduzem a
-- violação em 409 `directory_exists`. Pré-requisito: prod já deduplicada (o duplicado
-- de Ubatuba foi removido manualmente antes desta migração — ver CHANGELOG).
CREATE UNIQUE INDEX party_directory_territorio_key
    ON party_directory (org_id, party_sigla, esfera, uf, municipio)
    NULLS NOT DISTINCT;

COMMENT ON INDEX party_directory_territorio_key IS
    'dsoc-mandates: 1 diretório por (org, partido, esfera, uf, municipio); NULLS NOT DISTINCT cobre federal/estadual.';

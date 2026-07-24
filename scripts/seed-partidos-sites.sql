-- Seed dos sites oficiais dos partidos (0.41.0). Idempotente: só faz UPDATE
-- das rows de `party` que já existem na org default. Conjunto conservador —
-- apenas domínios de alta confiança; o super-admin (SOCRATES) completa/corrige
-- o resto pela UI de edição do partido.

\set org '11111111-1111-1111-1111-111111111111'

UPDATE party SET website = 'https://pt.org.br'                    WHERE org_id = :'org' AND sigla = 'PT';
UPDATE party SET website = 'https://www.mdb.org.br'               WHERE org_id = :'org' AND sigla = 'MDB';
UPDATE party SET website = 'https://www.psdb.org.br'              WHERE org_id = :'org' AND sigla = 'PSDB';
UPDATE party SET website = 'https://pdt.org.br'                   WHERE org_id = :'org' AND sigla = 'PDT';
UPDATE party SET website = 'https://pcdob.org.br'                 WHERE org_id = :'org' AND sigla = 'PCdoB';
UPDATE party SET website = 'https://novo.org.br'                  WHERE org_id = :'org' AND sigla = 'NOVO';
UPDATE party SET website = 'https://psol50.org.br'               WHERE org_id = :'org' AND sigla = 'PSOL';
UPDATE party SET website = 'https://pv.org.br'                    WHERE org_id = :'org' AND sigla = 'PV';
UPDATE party SET website = 'https://redesustentabilidade.org.br' WHERE org_id = :'org' AND sigla = 'REDE';
UPDATE party SET website = 'https://progressistas.org.br'        WHERE org_id = :'org' AND sigla = 'PP';
UPDATE party SET website = 'https://www.partidoliberal.org.br'   WHERE org_id = :'org' AND sigla = 'PL';
UPDATE party SET website = 'https://psb40.org.br'                WHERE org_id = :'org' AND sigla = 'PSB';
UPDATE party SET website = 'https://republicanos10.org.br'       WHERE org_id = :'org' AND sigla = 'REPUBLICANOS';
UPDATE party SET website = 'https://www.podemos.org.br'          WHERE org_id = :'org' AND sigla = 'PODE';
UPDATE party SET website = 'https://uniaobrasil.org.br'          WHERE org_id = :'org' AND sigla = 'UNIÃO';
UPDATE party SET website = 'https://solidariedade.org.br'        WHERE org_id = :'org' AND sigla = 'SOLIDARIEDADE';
UPDATE party SET website = 'https://avante.org.br'               WHERE org_id = :'org' AND sigla = 'AVANTE';
UPDATE party SET website = 'https://cidadania23.org.br'          WHERE org_id = :'org' AND sigla = 'CIDADANIA';

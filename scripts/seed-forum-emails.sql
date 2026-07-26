-- seed-forum-emails.sql — curadoria de e-mails institucionais (2026-07-26).
-- CADA endereço foi confirmado na página OFICIAL do órgão (fonte no comentário).
-- Órgãos que só atendem via Fala.BR/formulário ficam SEM e-mail (o patamar
-- cruza e fica pendente — futuro: integração Fala.BR).
-- Idempotente; re-executável. Rode com: psql ... -f scripts/seed-forum-emails.sql

BEGIN;

CREATE OR REPLACE FUNCTION pg_temp.set_email(p text, e text) RETURNS void LANGUAGE sql AS
$$ UPDATE forum SET contact_email = e WHERE full_path = p $$;

-- ============ SENADO — comissões (fonte: legis.senado.leg.br/atividade/comissoes/comissao/<id>)
SELECT pg_temp.set_email('senado/ccj',                      'ccj@senado.gov.br');   -- /34 (domínio antigo, como exibido)
SELECT pg_temp.set_email('senado/cae',                      'cae@senado.leg.br');   -- /38
SELECT pg_temp.set_email('senado/cas',                      'cas@senado.leg.br');   -- /40
SELECT pg_temp.set_email('senado/educacao',                 'ce@senado.leg.br');    -- /47
SELECT pg_temp.set_email('senado/meio-ambiente',            'cma@senado.leg.br');   -- /50
SELECT pg_temp.set_email('senado/direitos-humanos',         'cdh@senado.leg.br');   -- /834
SELECT pg_temp.set_email('senado/relacoes-exteriores',      'cre@senado.leg.br');   -- /54
SELECT pg_temp.set_email('senado/infraestrutura',           'ci@senado.leg.br');    -- /59
SELECT pg_temp.set_email('senado/desenvolvimento-regional', 'cdr@senado.leg.br');   -- /1306
SELECT pg_temp.set_email('senado/agricultura',              'cra@senado.gov.br');   -- /1307 (domínio antigo, como exibido)
SELECT pg_temp.set_email('senado/ciencia-tecnologia',       'cct@senado.leg.br');   -- /1363
SELECT pg_temp.set_email('senado/seguranca-publica',        'csp@senado.leg.br');   -- /2429
SELECT pg_temp.set_email('senado/transparencia',            'ctfc@senado.leg.br');  -- /1956
SELECT pg_temp.set_email('senado/esporte',                  'cesp@senado.leg.br');  -- /2615
SELECT pg_temp.set_email('senado/etica',                    'naot@senado.leg.br');  -- CEDP via NAOT (www25.senado.leg.br)

-- ============ MINISTÉRIOS — ouvidorias com e-mail público (fonte: gov.br/<órgão>/canais_atendimento)
SELECT pg_temp.set_email('ministerio-relacoes-exteriores',  'ouvidoria@itamaraty.gov.br');
SELECT pg_temp.set_email('ministerio-educacao',             'ouvidoria@mec.gov.br');
SELECT pg_temp.set_email('ministerio-previdencia',          'ouvidoria.mps@previdencia.gov.br');
SELECT pg_temp.set_email('ministerio-cidades',              'ouvidoria@cidades.gov.br');
SELECT pg_temp.set_email('ministerio-ciencia-tecnologia',   'ouvidoria@mcti.gov.br');
SELECT pg_temp.set_email('ministerio-comunicacoes',         'ouvidoria@mcom.gov.br');
SELECT pg_temp.set_email('ministerio-cultura',              'ouvidoriaminc@cultura.gov.br');
SELECT pg_temp.set_email('ministerio-integracao-regional',  'ouvidoria@mdr.gov.br');
SELECT pg_temp.set_email('ministerio-meio-ambiente',        'e-ouv@mma.gov.br');
SELECT pg_temp.set_email('ministerio-portos-aeroportos',    'ouvidoria@mpor.gov.br');
SELECT pg_temp.set_email('ministerio-transportes',          'ouvidoria@transportes.gov.br');
SELECT pg_temp.set_email('ministerio-turismo',              'ouvidoria@turismo.gov.br');
SELECT pg_temp.set_email('ministerio-direitos-humanos',     'ouvidoria@mdh.gov.br');
SELECT pg_temp.set_email('ministerio-povos-indigenas',      'mpi.ouv@povosindigenas.gov.br');
-- SEM e-mail público (só Fala.BR/formulário — ficam NULL de propósito):
-- fazenda, justica, defesa, saude (OuvSUS/136), trabalho, agricultura,
-- desenvolvimento-agrario, desenvolvimento-social, industria-comercio,
-- esporte, minas-energia, pesca, planejamento, gestao, igualdade-racial,
-- mulheres, empreendedorismo.

-- ============ JUDICIÁRIO — ouvidorias (fontes: portais oficiais de cada tribunal)
SELECT pg_temp.set_email('stf',   'ouvidoria@stf.jus.br');
SELECT pg_temp.set_email('stj',   'ouvidoria@stj.jus.br');
SELECT pg_temp.set_email('tst',   'ouvidoria@tst.jus.br');
SELECT pg_temp.set_email('tse',   'ouv@tse.jus.br');
SELECT pg_temp.set_email('stm',   'ouvidoria@stm.jus.br');
SELECT pg_temp.set_email('trf-2', 'ouvidoria@trf2.jus.br');
SELECT pg_temp.set_email('trf-3', 'ouvidoria@trf3.jus.br');
SELECT pg_temp.set_email('trf-4', 'ouvidoria@trf4.jus.br');
SELECT pg_temp.set_email('trf-5', 'ouvidoria@trf5.jus.br');
SELECT pg_temp.set_email('trf-6', 'ouvidoria@trf6.jus.br');
-- cnj: só formulário. trf-1: e-mail restrito a acesso SEI — ficam NULL.

COMMIT;

SELECT count(*) FILTER (WHERE contact_email IS NOT NULL) AS com_email,
       count(*) FILTER (WHERE contact_email IS NULL)     AS em_curadoria
FROM forum WHERE esfera = 'federal' OR kind = 'governanca';

-- seed-forums.sql — malha institucional dos fóruns (plano v3, 0540).
-- Idempotente: ON CONFLICT (org_id, full_path) DO NOTHING; re-executável.
--
-- Cria as RAÍZES: federal (senado+comissões, câmara+comissões, ministérios,
-- judiciário), governança, 27 estados e ~5.570 municípios (derivados da tabela
-- mandate). As 7 seções territoriais padrão NÃO são semeadas — materializam no
-- primeiro tópico (dsoc-forums::service::resolve_or_materialize).
--
-- E-mails institucionais: propositalmente NULL — curadoria humana via painel
-- admin (enviar cobrança pra endereço errado é pior que não enviar).

BEGIN;

CREATE OR REPLACE FUNCTION pg_temp.slugify(t text) RETURNS text LANGUAGE sql IMMUTABLE AS $$
  SELECT trim(BOTH '-' FROM regexp_replace(
    translate(lower(t),
      'áàâãäéèêëíìîïóòôõöúùûüçñ''',
      'aaaaaeeeeiiiiooooouuuucn-'),
    '[^a-z0-9]+', '-', 'g'))
$$;

-- ---------------------------------------------------------------- raízes federais
INSERT INTO forum (id, org_id, slug, full_path, name, description, kind, esfera, created_at)
SELECT gen_random_uuid(), o.id, v.slug, v.slug, v.name, v.descr, v.kind, v.esfera, now()
FROM org o,
(VALUES
  ('senado',     'Senado Federal',            'Debates dirigidos ao Senado e suas comissões.',            'institucional', 'federal'),
  ('camara',     'Câmara dos Deputados',      'Debates dirigidos à Câmara e suas comissões.',             'institucional', 'federal'),
  ('governanca', 'Governança da Plataforma',  'A governança da própria ferramenta democracia.social.br.', 'governanca',    NULL)
) AS v(slug, name, descr, kind, esfera)
WHERE o.id = '11111111-1111-1111-1111-111111111111'
ON CONFLICT (org_id, full_path) DO NOTHING;

-- ------------------------------------------------------------ comissões do Senado
INSERT INTO forum (id, org_id, parent_id, slug, full_path, name, kind, esfera, created_at)
SELECT gen_random_uuid(), p.org_id, p.id, v.slug, 'senado/' || v.slug, v.name,
       'institucional', 'federal', now()
FROM forum p,
(VALUES
  ('ccj',            'Comissão de Constituição, Justiça e Cidadania (CCJ)'),
  ('cae',            'Comissão de Assuntos Econômicos (CAE)'),
  ('cas',            'Comissão de Assuntos Sociais (CAS)'),
  ('educacao',       'Comissão de Educação e Cultura (CE)'),
  ('meio-ambiente',  'Comissão de Meio Ambiente (CMA)'),
  ('direitos-humanos','Comissão de Direitos Humanos e Legislação Participativa (CDH)'),
  ('relacoes-exteriores','Comissão de Relações Exteriores e Defesa Nacional (CRE)'),
  ('infraestrutura', 'Comissão de Serviços de Infraestrutura (CI)'),
  ('desenvolvimento-regional','Comissão de Desenvolvimento Regional e Turismo (CDR)'),
  ('agricultura',    'Comissão de Agricultura e Reforma Agrária (CRA)'),
  ('ciencia-tecnologia','Comissão de Ciência, Tecnologia, Inovação e Informática (CCT)'),
  ('seguranca-publica','Comissão de Segurança Pública (CSP)'),
  ('transparencia',  'Comissão de Transparência, Governança e Defesa do Consumidor (CTFC)'),
  ('esporte',        'Comissão de Esporte (CEsp)'),
  ('etica',          'Conselho de Ética e Decoro Parlamentar')
) AS v(slug, name)
WHERE p.full_path = 'senado'
ON CONFLICT (org_id, full_path) DO NOTHING;

-- ------------------------------------------------------------ comissões da Câmara
INSERT INTO forum (id, org_id, parent_id, slug, full_path, name, kind, esfera, created_at)
SELECT gen_random_uuid(), p.org_id, p.id, v.slug, 'camara/' || v.slug, v.name,
       'institucional', 'federal', now()
FROM forum p,
(VALUES
  ('ccjc',           'Comissão de Constituição e Justiça e de Cidadania (CCJC)'),
  ('financas',       'Comissão de Finanças e Tributação (CFT)'),
  ('saude',          'Comissão de Saúde (CSAÚDE)'),
  ('previdencia',    'Comissão de Previdência, Assistência Social e Família (CPASF)'),
  ('educacao',       'Comissão de Educação (CE)'),
  ('cultura',        'Comissão de Cultura (CCULT)'),
  ('agricultura',    'Comissão de Agricultura, Pecuária, Abastecimento e Des. Rural (CAPADR)'),
  ('meio-ambiente',  'Comissão de Meio Ambiente e Desenvolvimento Sustentável (CMADS)'),
  ('ciencia-tecnologia','Comissão de Ciência, Tecnologia e Inovação (CCTI)'),
  ('comunicacao',    'Comissão de Comunicação (CCOM)'),
  ('defesa-consumidor','Comissão de Defesa do Consumidor (CDC)'),
  ('desenvolvimento-urbano','Comissão de Desenvolvimento Urbano (CDU)'),
  ('direitos-humanos','Comissão de Direitos Humanos, Minorias e Igualdade Racial (CDHMIR)'),
  ('relacoes-exteriores','Comissão de Relações Exteriores e de Defesa Nacional (CREDN)'),
  ('minas-energia',  'Comissão de Minas e Energia (CME)'),
  ('seguranca-publica','Comissão de Segurança Pública e Combate ao Crime Organizado (CSPCCO)'),
  ('trabalho',       'Comissão de Trabalho (CTRAB)'),
  ('viacao-transportes','Comissão de Viação e Transportes (CVT)'),
  ('turismo',        'Comissão de Turismo (CTUR)'),
  ('esporte',        'Comissão do Esporte (CESPO)'),
  ('fiscalizacao',   'Comissão de Fiscalização Financeira e Controle (CFFC)'),
  ('industria-comercio','Comissão de Indústria, Comércio e Serviços (CICS)'),
  ('integracao-nacional','Comissão de Integração Nacional e Desenvolvimento Regional (CINDR)'),
  ('legislacao-participativa','Comissão de Legislação Participativa (CLP)'),
  ('mulheres',       'Comissão de Defesa dos Direitos da Mulher (CMULHER)'),
  ('idoso',          'Comissão de Defesa dos Direitos da Pessoa Idosa (CIDOSO)'),
  ('pessoa-com-deficiencia','Comissão de Defesa dos Direitos das Pessoas com Deficiência (CPD)'),
  ('amazonia',       'Comissão da Amazônia e dos Povos Originários e Tradicionais (CAPOT)'),
  ('administracao',  'Comissão de Administração e Serviço Público (CASP)'),
  ('etica',          'Conselho de Ética e Decoro Parlamentar')
) AS v(slug, name)
WHERE p.full_path = 'camara'
ON CONFLICT (org_id, full_path) DO NOTHING;

-- ---------------------------------------------------------------- ministérios
INSERT INTO forum (id, org_id, slug, full_path, name, kind, esfera, created_at)
SELECT gen_random_uuid(), o.id,
       'ministerio-' || COALESCE(v.slug, pg_temp.slugify(v.name)),
       'ministerio-' || COALESCE(v.slug, pg_temp.slugify(v.name)),
       'Ministério ' || v.prefixo || ' ' || v.name,
       'institucional', 'federal', now()
FROM org o,
(VALUES
  ('Fazenda', 'da', NULL), ('Justiça e Segurança Pública', 'da', 'justica'),
  ('Defesa', 'da', NULL), ('Relações Exteriores', 'das', NULL),
  ('Saúde', 'da', NULL), ('Educação', 'da', NULL),
  ('Trabalho e Emprego', 'do', 'trabalho'), ('Previdência Social', 'da', 'previdencia'),
  ('Agricultura e Pecuária', 'da', 'agricultura'),
  ('Desenvolvimento Agrário e Agricultura Familiar', 'do', 'desenvolvimento-agrario'),
  ('Desenvolvimento e Assistência Social, Família e Combate à Fome', 'do', 'desenvolvimento-social'),
  ('Cidades', 'das', NULL), ('Ciência, Tecnologia e Inovação', 'da', 'ciencia-tecnologia'),
  ('Comunicações', 'das', NULL), ('Cultura', 'da', NULL),
  ('Desenvolvimento, Indústria, Comércio e Serviços', 'do', 'industria-comercio'),
  ('Esporte', 'do', NULL), ('Integração e Desenvolvimento Regional', 'da', 'integracao-regional'),
  ('Meio Ambiente e Mudança do Clima', 'do', 'meio-ambiente'),
  ('Minas e Energia', 'de', NULL), ('Pesca e Aquicultura', 'da', 'pesca'),
  ('Planejamento e Orçamento', 'do', 'planejamento'),
  ('Portos e Aeroportos', 'de', 'portos-aeroportos'), ('Transportes', 'dos', NULL),
  ('Turismo', 'do', NULL), ('Gestão e Inovação em Serviços Públicos', 'da', 'gestao'),
  ('Igualdade Racial', 'da', NULL), ('Mulheres', 'das', NULL),
  ('Direitos Humanos e Cidadania', 'dos', 'direitos-humanos'),
  ('Povos Indígenas', 'dos', NULL),
  ('Empreendedorismo, Microempresa e Empresa de Pequeno Porte', 'do', 'empreendedorismo')
) AS v(name, prefixo, slug)
WHERE o.id = '11111111-1111-1111-1111-111111111111'
ON CONFLICT (org_id, full_path) DO NOTHING;

-- ---------------------------------------------------------------- judiciário federal
INSERT INTO forum (id, org_id, slug, full_path, name, kind, esfera, created_at)
SELECT gen_random_uuid(), o.id, v.slug, v.slug, v.name, 'institucional', 'federal', now()
FROM org o,
(VALUES
  ('stf',  'Supremo Tribunal Federal (STF)'),
  ('stj',  'Superior Tribunal de Justiça (STJ)'),
  ('tst',  'Tribunal Superior do Trabalho (TST)'),
  ('tse',  'Tribunal Superior Eleitoral (TSE)'),
  ('stm',  'Superior Tribunal Militar (STM)'),
  ('cnj',  'Conselho Nacional de Justiça (CNJ)'),
  ('trf-1','Tribunal Regional Federal da 1ª Região'),
  ('trf-2','Tribunal Regional Federal da 2ª Região'),
  ('trf-3','Tribunal Regional Federal da 3ª Região'),
  ('trf-4','Tribunal Regional Federal da 4ª Região'),
  ('trf-5','Tribunal Regional Federal da 5ª Região'),
  ('trf-6','Tribunal Regional Federal da 6ª Região')
) AS v(slug, name)
WHERE o.id = '11111111-1111-1111-1111-111111111111'
ON CONFLICT (org_id, full_path) DO NOTHING;

-- ---------------------------------------------------------------- estados (27)
INSERT INTO forum (id, org_id, slug, full_path, name, kind, esfera, uf, created_at)
SELECT gen_random_uuid(), o.id, lower(v.uf), lower(v.uf), v.name,
       'institucional', 'estadual', v.uf, now()
FROM org o,
(VALUES
  ('AC','Acre'),('AL','Alagoas'),('AP','Amapá'),('AM','Amazonas'),('BA','Bahia'),
  ('CE','Ceará'),('DF','Distrito Federal'),('ES','Espírito Santo'),('GO','Goiás'),
  ('MA','Maranhão'),('MT','Mato Grosso'),('MS','Mato Grosso do Sul'),('MG','Minas Gerais'),
  ('PA','Pará'),('PB','Paraíba'),('PR','Paraná'),('PE','Pernambuco'),('PI','Piauí'),
  ('RJ','Rio de Janeiro'),('RN','Rio Grande do Norte'),('RS','Rio Grande do Sul'),
  ('RO','Rondônia'),('RR','Roraima'),('SC','Santa Catarina'),('SP','São Paulo'),
  ('SE','Sergipe'),('TO','Tocantins')
) AS v(uf, name)
WHERE o.id = '11111111-1111-1111-1111-111111111111'
ON CONFLICT (org_id, full_path) DO NOTHING;

-- ------------------------------------------- municípios (derivados de mandate)
INSERT INTO forum (id, org_id, parent_id, slug, full_path, name, kind, esfera, uf, municipio, created_at)
SELECT gen_random_uuid(), p.org_id, p.id,
       pg_temp.slugify(m.municipio),
       p.slug || '/' || pg_temp.slugify(m.municipio),
       initcap(lower(m.municipio)),
       'institucional', 'municipal', m.uf, m.municipio, now()
FROM (SELECT DISTINCT uf, municipio FROM mandate
       WHERE sphere = 'municipal' AND uf IS NOT NULL AND municipio IS NOT NULL) m
JOIN forum p ON p.esfera = 'estadual' AND p.uf = m.uf AND p.parent_id IS NULL
WHERE pg_temp.slugify(m.municipio) <> ''
ON CONFLICT (org_id, full_path) DO NOTHING;

COMMIT;

SELECT kind, esfera, count(*) FROM forum GROUP BY 1, 2 ORDER BY 1, 2;

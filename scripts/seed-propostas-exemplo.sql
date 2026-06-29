-- Seed de propostas exemplo dirigidas a parlamentares reais já no banco. Cria uma cidadã
-- "Curadora DemocraciaBR" como autora e gera 8 propostas com volumes de apoio variados pra a
-- página ter movimento sem precisar de testes manuais ponta-a-ponta primeiro.
-- Idempotente: usa fixed UUIDs (re-run só dá ON CONFLICT DO NOTHING).

BEGIN;

-- 1) Cidadã-curadora autora das propostas exemplo. Existe localmente, perfil público mas SEM
--    federation (não tem auth_credential — não é login real, é só atribuição visual).
INSERT INTO citizen (id, org_id, oidc_subject, verification_level, created_at,
                     display_name, handle, bio, is_public)
VALUES ('019f0700-0000-0000-0000-000000000001',
        '11111111-1111-1111-1111-111111111111',
        'seed:curadora',
        'directory',
        now(),
        'Curadora DemocraciaBR',
        'curadora',
        'Exemplos de demandas cívicas curados pela plataforma.',
        true)
ON CONFLICT (id) DO NOTHING;

-- 2) 8 propostas. Para cada uma escolhemos um parlamentar real (lookup pelo display_name).
--    Cada INSERT é guarded por WHERE NOT EXISTS para idempotência sem precisar de UNIQUE.

INSERT INTO proposal (id, org_id, mandate_id, title, body, threshold, support_count,
                      author_citizen_id, status, created_at)
SELECT '019f0700-1000-7000-0000-000000000001',
       '11111111-1111-1111-1111-111111111111',
       m.id,
       'Investigar contratos suspeitos da merenda escolar no DF',
       'Solicitamos abertura de requerimento de informações sobre os contratos de fornecimento de merenda escolar firmados nos últimos 24 meses pela Secretaria de Educação do Distrito Federal. Há indícios sólidos de superfaturamento e direcionamento. A população precisa de transparência total.',
       100, 47,
       '019f0700-0000-0000-0000-000000000001',
       'published',
       now() - interval '5 days'
  FROM mandate m
 WHERE m.display_name ILIKE 'Erika Hilton%' AND m.party = 'PSOL'
 LIMIT 1
ON CONFLICT (id) DO NOTHING;

INSERT INTO proposal (id, org_id, mandate_id, title, body, threshold, support_count,
                      author_citizen_id, status, created_at)
SELECT '019f0700-1000-7000-0000-000000000002',
       '11111111-1111-1111-1111-111111111111',
       m.id, 'Apoie a PEC dos Servidores Públicos da Educação',
       'Pedimos voto favorável à PEC que garante o piso nacional do magistério com correção anual pela inflação. Professores da rede pública há anos esperam essa atualização. O texto está pronto pra votação em segundo turno.',
       150, 132,
       '019f0700-0000-0000-0000-000000000001',
       'published',
       now() - interval '12 days'
  FROM mandate m
 WHERE m.party = 'PT' AND m.house = 'camara'
 ORDER BY random()
 LIMIT 1
ON CONFLICT (id) DO NOTHING;

INSERT INTO proposal (id, org_id, mandate_id, title, body, threshold, support_count,
                      author_citizen_id, status, created_at)
SELECT '019f0700-1000-7000-0000-000000000003',
       '11111111-1111-1111-1111-111111111111',
       m.id, 'Fiscalize a obra da BR-364 no Acre',
       'A obra de duplicação da BR-364 entre Rio Branco e Porto Velho está atrasada em mais de 18 meses. Pedimos audiência pública e relatório de comissão para apurar onde o dinheiro está sendo gasto.',
       80, 12,
       '019f0700-0000-0000-0000-000000000001',
       'published',
       now() - interval '2 days'
  FROM mandate m
 WHERE m.party = 'PT' AND m.house = 'senado'
 ORDER BY random()
 LIMIT 1
ON CONFLICT (id) DO NOTHING;

INSERT INTO proposal (id, org_id, mandate_id, title, body, threshold, support_count,
                      author_citizen_id, status, created_at)
SELECT '019f0700-1000-7000-0000-000000000004',
       '11111111-1111-1111-1111-111111111111',
       m.id, 'Defender o SUS contra cortes no orçamento de 2027',
       'O relatório preliminar da LOA 2027 prevê corte de 8% no SUS. Pedimos posicionamento público contra o corte e apresentação de emenda restaurando o piso constitucional. O SUS é prioridade.',
       200, 178,
       '019f0700-0000-0000-0000-000000000001',
       'published',
       now() - interval '8 days'
  FROM mandate m
 WHERE m.party = 'PCdoB'
 ORDER BY random()
 LIMIT 1
ON CONFLICT (id) DO NOTHING;

INSERT INTO proposal (id, org_id, mandate_id, title, body, threshold, support_count,
                      author_citizen_id, status, created_at)
SELECT '019f0700-1000-7000-0000-000000000005',
       '11111111-1111-1111-1111-111111111111',
       m.id, 'Proteger Áreas de Proteção Ambiental urbanas',
       'Sugerimos audiência pública com a sociedade civil para discutir o avanço da especulação imobiliária em APAs urbanas. Precisamos de relator(a) sensível ao tema apresentando o requerimento.',
       60, 28,
       '019f0700-0000-0000-0000-000000000001',
       'published',
       now() - interval '4 days'
  FROM mandate m
 WHERE m.party = 'PV'
 ORDER BY random()
 LIMIT 1
ON CONFLICT (id) DO NOTHING;

INSERT INTO proposal (id, org_id, mandate_id, title, body, threshold, support_count,
                      author_citizen_id, status, created_at)
SELECT '019f0700-1000-7000-0000-000000000006',
       '11111111-1111-1111-1111-111111111111',
       m.id, 'Justiça pelas famílias atingidas pelas chuvas no RS',
       'Pedimos prioridade na tramitação do projeto que cria fundo de reconstrução permanente para o Rio Grande do Sul. Famílias que perderam tudo em 2024 e 2025 continuam sem moradia definitiva.',
       300, 412,
       '019f0700-0000-0000-0000-000000000001',
       'published',
       now() - interval '15 days'
  FROM mandate m
 WHERE m.uf = 'RS' AND m.party = 'PT'
 ORDER BY random()
 LIMIT 1
ON CONFLICT (id) DO NOTHING;

INSERT INTO proposal (id, org_id, mandate_id, title, body, threshold, support_count,
                      author_citizen_id, status, created_at)
SELECT '019f0700-1000-7000-0000-000000000007',
       '11111111-1111-1111-1111-111111111111',
       m.id, 'Audiência pública sobre violência policial em comunidades',
       'Solicitamos convocação de audiência pública com representantes das comunidades atingidas, MP, Defensoria e comando da PM para discutir protocolos e responsabilização. As mortes precisam ser investigadas com transparência.',
       150, 67,
       '019f0700-0000-0000-0000-000000000001',
       'published',
       now() - interval '6 days'
  FROM mandate m
 WHERE m.party = 'PSOL'
 ORDER BY random()
 LIMIT 1
ON CONFLICT (id) DO NOTHING;

INSERT INTO proposal (id, org_id, mandate_id, title, body, threshold, support_count,
                      author_citizen_id, status, created_at)
SELECT '019f0700-1000-7000-0000-000000000008',
       '11111111-1111-1111-1111-111111111111',
       m.id, 'Por uma reforma tributária que cobre dos mais ricos',
       'A regulamentação da reforma tributária precisa garantir alíquotas progressivas sobre grandes fortunas e dividendos. Pedimos apoio ao texto substitutivo apresentado pela bancada que prioriza justiça fiscal.',
       250, 195,
       '019f0700-0000-0000-0000-000000000001',
       'published',
       now() - interval '10 days'
  FROM mandate m
 WHERE m.party = 'REDE'
 ORDER BY random()
 LIMIT 1
ON CONFLICT (id) DO NOTHING;

COMMIT;

-- Confirma:
SELECT count(*) AS total_de_propostas_exemplo
  FROM proposal
 WHERE author_citizen_id = '019f0700-0000-0000-0000-000000000001';

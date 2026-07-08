-- Seed idempotente de candidaturas-exemplo pra 2026 (fictícias, com nomes
-- diferentes de mandatos reais). Desbloqueia o fluxo `/propor` até o import
-- de dados TSE real. Todos com is_candidate=true pra aparecer no picker.
--
-- Uniques: source='manual' + source_external_id específico por
-- linha → re-run apenas atualiza campos, não duplica.
--
-- Rodar em prod:
--   sudo -u postgres psql democracia_social -f seed-candidatos-exemplo-2026.sql

BEGIN;

-- Todos vinculados à org 11111...1 (seed default) e sphere federal/estadual.
-- office textual segue o padrão dos mandatos reais (DEPUTADO, SENADOR, etc.).

INSERT INTO mandate (
    id, org_id, office, display_name, public_email, is_candidate,
    party, uf, house, sphere, source, source_external_id, created_at
) VALUES
-- === Federais ===
(gen_random_uuid(), '11111111-1111-1111-1111-111111111111'::uuid,
 'DEPUTADO_FEDERAL', 'Candidata Exemplo Fed 1 (SP)',
 'candidato1@example.br', true, 'PT', 'SP', 'camara', 'federal',
 'manual', 'seed-candidato-2026-fed-01', now()),
(gen_random_uuid(), '11111111-1111-1111-1111-111111111111'::uuid,
 'DEPUTADO_FEDERAL', 'Candidato Exemplo Fed 2 (RJ)',
 'candidato2@example.br', true, 'PSOL', 'RJ', 'camara', 'federal',
 'manual', 'seed-candidato-2026-fed-02', now()),
(gen_random_uuid(), '11111111-1111-1111-1111-111111111111'::uuid,
 'DEPUTADO_FEDERAL', 'Candidata Exemplo Fed 3 (MG)',
 'candidato3@example.br', true, 'PSDB', 'MG', 'camara', 'federal',
 'manual', 'seed-candidato-2026-fed-03', now()),
(gen_random_uuid(), '11111111-1111-1111-1111-111111111111'::uuid,
 'SENADOR', 'Candidato Exemplo Sen 1 (SP)',
 'senador1@example.br', true, 'MDB', 'SP', 'senado', 'federal',
 'manual', 'seed-candidato-2026-sen-01', now()),
(gen_random_uuid(), '11111111-1111-1111-1111-111111111111'::uuid,
 'SENADOR', 'Candidata Exemplo Sen 2 (BA)',
 'senador2@example.br', true, 'REDE', 'BA', 'senado', 'federal',
 'manual', 'seed-candidato-2026-sen-02', now()),
-- === Estaduais ===
(gen_random_uuid(), '11111111-1111-1111-1111-111111111111'::uuid,
 'DEPUTADO_ESTADUAL', 'Candidato Exemplo Est 1 (SP)',
 'estadual1@example.br', true, 'PL', 'SP', 'assembleia', 'estadual',
 'manual', 'seed-candidato-2026-est-01', now()),
(gen_random_uuid(), '11111111-1111-1111-1111-111111111111'::uuid,
 'DEPUTADO_ESTADUAL', 'Candidata Exemplo Est 2 (RS)',
 'estadual2@example.br', true, 'PT', 'RS', 'assembleia', 'estadual',
 'manual', 'seed-candidato-2026-est-02', now()),
(gen_random_uuid(), '11111111-1111-1111-1111-111111111111'::uuid,
 'DEPUTADO_ESTADUAL', 'Candidato Exemplo Est 3 (PR)',
 'estadual3@example.br', true, 'PSDB', 'PR', 'assembleia', 'estadual',
 'manual', 'seed-candidato-2026-est-03', now()),
(gen_random_uuid(), '11111111-1111-1111-1111-111111111111'::uuid,
 'GOVERNADOR', 'Candidata Exemplo Gov 1 (RJ)',
 'gov1@example.br', true, 'PDT', 'RJ', NULL, 'estadual',
 'manual', 'seed-candidato-2026-gov-01', now()),
(gen_random_uuid(), '11111111-1111-1111-1111-111111111111'::uuid,
 'GOVERNADOR', 'Candidato Exemplo Gov 2 (MG)',
 'gov2@example.br', true, 'NOVO', 'MG', NULL, 'estadual',
 'manual', 'seed-candidato-2026-gov-02', now())
ON CONFLICT (source, source_external_id) WHERE source IS NOT NULL AND source_external_id IS NOT NULL
DO UPDATE SET
    display_name = EXCLUDED.display_name,
    party        = EXCLUDED.party,
    uf           = EXCLUDED.uf,
    house        = EXCLUDED.house,
    office       = EXCLUDED.office,
    sphere       = EXCLUDED.sphere,
    is_candidate = EXCLUDED.is_candidate;

-- Resumo
SELECT sphere, count(*) FROM mandate
 WHERE is_candidate=true AND source='manual'
 GROUP BY sphere;

COMMIT;

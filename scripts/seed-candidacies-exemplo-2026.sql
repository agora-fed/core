-- Seed idempotente de candidacies-exemplo pra as 4 elections estruturais.
-- Espelha os 10 mandate.is_candidate=true que foram semeados por
-- `seed-candidatos-exemplo-2026.sql`. Vinculação via mandate_id.
--
-- Elections IDs (do seed 0505):
--   fed 1t: a0000001-0000-4000-8000-000020261001
--   fed 2t: a0000002-0000-4000-8000-000020261002
--   est 1t: a0000003-0000-4000-8000-000020261003
--   est 2t: a0000004-0000-4000-8000-000020261004
--
-- Idempotência: PK explícita = SHA hex('seed-cand-<sigla>'). Re-run atualiza
-- por INSERT ... ON CONFLICT (id) DO UPDATE.
--
-- Números eleitorais são fictícios mas realistas (2 dígitos = partido para
-- deputado, mesma sigla real; 3-5 dígitos para outros cargos).

BEGIN;

WITH mandate_by_sig AS (
  SELECT source_external_id, id FROM mandate
   WHERE source = 'manual'
     AND source_external_id LIKE 'seed-candidato-2026-%'
)
INSERT INTO candidacy (
  id, election_id, mandate_id, party_sigla, office, number,
  sphere_uf, candidate_name, candidate_gender, status, created_at
) VALUES
  -- Federais 1º turno
  ('c0000001-0000-4000-8000-000000000001',
   'a0000001-0000-4000-8000-000020261001',
   (SELECT id FROM mandate_by_sig WHERE source_external_id='seed-candidato-2026-fed-01'),
   'PT', 'DEPUTADO_FEDERAL', '13001', 'SP', 'Candidata Exemplo Fed 1 (SP)', 'mulher', 'ativa', now()),
  ('c0000001-0000-4000-8000-000000000002',
   'a0000001-0000-4000-8000-000020261001',
   (SELECT id FROM mandate_by_sig WHERE source_external_id='seed-candidato-2026-fed-02'),
   'PSOL', 'DEPUTADO_FEDERAL', '50001', 'RJ', 'Candidato Exemplo Fed 2 (RJ)', 'homem', 'ativa', now()),
  ('c0000001-0000-4000-8000-000000000003',
   'a0000001-0000-4000-8000-000020261001',
   (SELECT id FROM mandate_by_sig WHERE source_external_id='seed-candidato-2026-fed-03'),
   'PSDB', 'DEPUTADO_FEDERAL', '45001', 'MG', 'Candidata Exemplo Fed 3 (MG)', 'mulher', 'ativa', now()),
  ('c0000001-0000-4000-8000-000000000004',
   'a0000001-0000-4000-8000-000020261001',
   (SELECT id FROM mandate_by_sig WHERE source_external_id='seed-candidato-2026-sen-01'),
   'MDB', 'SENADOR', '150', 'SP', 'Candidato Exemplo Sen 1 (SP)', 'homem', 'ativa', now()),
  ('c0000001-0000-4000-8000-000000000005',
   'a0000001-0000-4000-8000-000020261001',
   (SELECT id FROM mandate_by_sig WHERE source_external_id='seed-candidato-2026-sen-02'),
   'REDE', 'SENADOR', '180', 'BA', 'Candidata Exemplo Sen 2 (BA)', 'mulher', 'ativa', now()),
  -- Estaduais 1º turno
  ('c0000001-0000-4000-8000-000000000006',
   'a0000003-0000-4000-8000-000020261003',
   (SELECT id FROM mandate_by_sig WHERE source_external_id='seed-candidato-2026-est-01'),
   'PL', 'DEPUTADO_ESTADUAL', '22001', 'SP', 'Candidato Exemplo Est 1 (SP)', 'homem', 'ativa', now()),
  ('c0000001-0000-4000-8000-000000000007',
   'a0000003-0000-4000-8000-000020261003',
   (SELECT id FROM mandate_by_sig WHERE source_external_id='seed-candidato-2026-est-02'),
   'PT', 'DEPUTADO_ESTADUAL', '13002', 'RS', 'Candidata Exemplo Est 2 (RS)', 'mulher', 'ativa', now()),
  ('c0000001-0000-4000-8000-000000000008',
   'a0000003-0000-4000-8000-000020261003',
   (SELECT id FROM mandate_by_sig WHERE source_external_id='seed-candidato-2026-est-03'),
   'PSDB', 'DEPUTADO_ESTADUAL', '45002', 'PR', 'Candidato Exemplo Est 3 (PR)', 'homem', 'ativa', now()),
  ('c0000001-0000-4000-8000-000000000009',
   'a0000003-0000-4000-8000-000020261003',
   (SELECT id FROM mandate_by_sig WHERE source_external_id='seed-candidato-2026-gov-01'),
   'PDT', 'GOVERNADOR', '12', 'RJ', 'Candidata Exemplo Gov 1 (RJ)', 'mulher', 'ativa', now()),
  ('c0000001-0000-4000-8000-000000000010',
   'a0000003-0000-4000-8000-000020261003',
   (SELECT id FROM mandate_by_sig WHERE source_external_id='seed-candidato-2026-gov-02'),
   'NOVO', 'GOVERNADOR', '30', 'MG', 'Candidato Exemplo Gov 2 (MG)', 'homem', 'ativa', now())
ON CONFLICT (id) DO UPDATE SET
    mandate_id       = EXCLUDED.mandate_id,
    party_sigla      = EXCLUDED.party_sigla,
    candidate_name   = EXCLUDED.candidate_name,
    candidate_gender = EXCLUDED.candidate_gender,
    status           = EXCLUDED.status;

SELECT c.office, c.party_sigla, c.sphere_uf, c.candidate_name
  FROM candidacy c
 WHERE c.id::text LIKE 'c0000001-%'
 ORDER BY c.office, c.candidate_name;

COMMIT;

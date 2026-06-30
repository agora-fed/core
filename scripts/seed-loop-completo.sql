-- Força o consequence loop a rodar PRA VALER: cruza o threshold de 3 propostas seedadas,
-- cria SLA pra cada uma, e simula 3 estados diferentes do placar:
--   1. SLA correndo (pending, prazo de 7 dias)
--   2. SLA já respondida (answered, com consequence_response real)
--   3. SLA expirada (ignored — silêncio público)
--
-- Também vincula a cidadã `esposa.teste` ao mandato da resposta #2 para que ela possa testar
-- o painel-mandato em primeira pessoa.

BEGIN;

-- ============================================================================
-- 0) Subir a verificação da Esposa pra `directory` (a respond do consequence
--    exige nível >= Directory). Faz isso só pra essa cidadã; outras seguem normais.
-- ============================================================================
UPDATE citizen SET verification_level = 'directory'
 WHERE handle = 'esposa.teste';

-- ============================================================================
-- 1) SLA CORRENDO: "Defender o SUS contra cortes" (PCdoB, 178/200 apoios).
--    Cruzamos o threshold artificialmente e criamos uma SLA com prazo de 7 dias.
-- ============================================================================
UPDATE proposal
   SET support_count = threshold,
       threshold_crossed_at = now() - interval '1 day',
       published_at = COALESCE(published_at, now() - interval '8 days'),
       status = 'clustered'
 WHERE id = '019f0700-1000-7000-0000-000000000004';

INSERT INTO consequence_sla
    (id, org_id, mandate_id, cluster_id, proposal_id, status, started_at, due_at, created_at)
SELECT gen_random_uuid(),
       p.org_id,
       p.mandate_id,
       COALESCE(p.cluster_id, gen_random_uuid()),
       p.id,
       'pending',
       now() - interval '1 day',
       now() + interval '6 days 23 hours',
       now() - interval '1 day'
  FROM proposal p
 WHERE p.id = '019f0700-1000-7000-0000-000000000004'
   AND NOT EXISTS (SELECT 1 FROM consequence_sla WHERE proposal_id = p.id);

-- ============================================================================
-- 2) SLA RESPONDIDA: "Apoie a PEC dos Servidores" (PT/Câmara, 132/150).
--    Vincula a Esposa como operadora desse mandato e cria uma SLA já respondida.
-- ============================================================================
UPDATE proposal
   SET support_count = threshold,
       threshold_crossed_at = now() - interval '10 days',
       published_at = COALESCE(published_at, now() - interval '15 days'),
       status = 'clustered'
 WHERE id = '019f0700-1000-7000-0000-000000000002';

-- Vincula Esposa ao mandato dessa proposta (PEC dos Servidores).
INSERT INTO mandate_identity_binding
    (id, mandate_id, verification_level, evidence_ref, verified_at, created_at, citizen_id)
SELECT gen_random_uuid(),
       p.mandate_id,
       'directory',
       'seed:loop-completo',
       now() - interval '20 days',
       now() - interval '20 days',
       c.id
  FROM proposal p
       JOIN citizen c ON c.handle = 'esposa.teste'
 WHERE p.id = '019f0700-1000-7000-0000-000000000002'
   AND NOT EXISTS (
     SELECT 1 FROM mandate_identity_binding b
      WHERE b.mandate_id = p.mandate_id AND b.citizen_id = c.id
   );

-- SLA respondida há 3 dias (started_at -10d, due +14d, mas already 'answered').
INSERT INTO consequence_sla
    (id, org_id, mandate_id, cluster_id, proposal_id, status, started_at, due_at, created_at)
SELECT '019f0700-2000-7000-0000-000000000002',
       p.org_id,
       p.mandate_id,
       COALESCE(p.cluster_id, gen_random_uuid()),
       p.id,
       'answered',
       now() - interval '10 days',
       now() + interval '4 days',
       now() - interval '10 days'
  FROM proposal p
 WHERE p.id = '019f0700-1000-7000-0000-000000000002'
   AND NOT EXISTS (SELECT 1 FROM consequence_sla WHERE proposal_id = p.id);

INSERT INTO consequence_response (id, sla_id, mandate_id, body, committed, responded_at, created_at)
SELECT gen_random_uuid(),
       s.id,
       s.mandate_id,
       'Apoio a PEC. Já assinei o requerimento e estou conversando com a relatora pra acelerar a votação em segundo turno. Atualizarei aqui semanalmente.',
       true,
       now() - interval '3 days',
       now() - interval '3 days'
  FROM consequence_sla s
 WHERE s.id = '019f0700-2000-7000-0000-000000000002'
   AND NOT EXISTS (SELECT 1 FROM consequence_response WHERE sla_id = s.id);

-- ============================================================================
-- 3) SLA EXPIRADA (silêncio público): "Justiça pelas famílias atingidas pelas chuvas no RS"
--    (PT/RS, 412/300 — passou bem do threshold). SLA criada há 30 dias com prazo de 14d.
-- ============================================================================
UPDATE proposal
   SET threshold_crossed_at = now() - interval '30 days',
       published_at = COALESCE(published_at, now() - interval '35 days'),
       status = 'clustered'
 WHERE id = '019f0700-1000-7000-0000-000000000006';

INSERT INTO consequence_sla
    (id, org_id, mandate_id, cluster_id, proposal_id, status, started_at, due_at, created_at)
SELECT gen_random_uuid(),
       p.org_id,
       p.mandate_id,
       COALESCE(p.cluster_id, gen_random_uuid()),
       p.id,
       'ignored',
       now() - interval '30 days',
       now() - interval '16 days',
       now() - interval '30 days'
  FROM proposal p
 WHERE p.id = '019f0700-1000-7000-0000-000000000006'
   AND NOT EXISTS (SELECT 1 FROM consequence_sla WHERE proposal_id = p.id);

-- ============================================================================
-- 4) Scorecard projection for the three affected mandates.
--    The scorecard worker normally builds these from events, but we're injecting state
--    directly — so we project the row entries by hand to make the placar reflect reality.
-- ============================================================================
INSERT INTO scorecard (id, org_id, mandate_id, answered, ignored, created_at, updated_at)
SELECT gen_random_uuid(), s.org_id, s.mandate_id,
       COUNT(*) FILTER (WHERE s.status IN ('answered','acted')),
       COUNT(*) FILTER (WHERE s.status = 'ignored'),
       now(), now()
  FROM consequence_sla s
 WHERE NOT EXISTS (SELECT 1 FROM scorecard sc WHERE sc.mandate_id = s.mandate_id)
 GROUP BY s.org_id, s.mandate_id;

INSERT INTO scorecard_entry (id, scorecard_id, sla_id, outcome, response_hours, occurred_at, created_at)
SELECT gen_random_uuid(),
       sc.id,
       s.id,
       -- The CHECK only allows 'answered' | 'ignored'; collapse 'acted' (response with commitment)
       -- onto 'answered' for the placar projection.
       CASE WHEN s.status IN ('answered', 'acted') THEN 'answered'
            ELSE 'ignored' END,
       CASE WHEN s.status IN ('answered', 'acted')
            THEN GREATEST(0.0, EXTRACT(epoch FROM (now() - interval '3 days' - s.started_at)) / 3600.0)
            ELSE NULL END,
       CASE WHEN s.status IN ('answered', 'acted') THEN now() - interval '3 days'
            ELSE s.due_at END,
       now()
  FROM consequence_sla s
       JOIN scorecard sc ON sc.mandate_id = s.mandate_id
 WHERE s.status IN ('answered', 'acted', 'ignored')
   AND NOT EXISTS (SELECT 1 FROM scorecard_entry e WHERE e.sla_id = s.id);

COMMIT;

-- Resumo
SELECT 'SLAs no DB' AS metric, count(*)::text AS value FROM consequence_sla
UNION ALL SELECT 'SLA pending', count(*)::text FROM consequence_sla WHERE status = 'pending'
UNION ALL SELECT 'SLA answered', count(*)::text FROM consequence_sla WHERE status = 'answered'
UNION ALL SELECT 'SLA ignored', count(*)::text FROM consequence_sla WHERE status = 'ignored'
UNION ALL SELECT 'consequence_response', count(*)::text FROM consequence_response
UNION ALL SELECT 'scorecard_entry', count(*)::text FROM scorecard_entry
UNION ALL SELECT 'esposa.teste vinculada?', verification_level FROM citizen WHERE handle = 'esposa.teste';

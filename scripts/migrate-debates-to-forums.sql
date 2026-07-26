-- migrate-debates-to-forums.sql — fusão DEBATE→FÓRUM (issue #19, 2026-07-26).
--
-- Migra o conteúdo da plataforma de Debates (encerrada) para tópicos de fórum.
-- Estado real de prod: 1 debate ("Transporte público deve ser gratuito?",
-- nacional) com 4 contribuições (2 pro, 1 con, 1 neutral) → vira tópico em
-- /f/ministerio-transportes (tema: transporte; esfera federal/nacional).
--
-- Mapeamento de posições: pro→favor, con→contra, neutral→ponderacao.
-- Autor do tópico = autor da contribuição mais antiga. Datas preservadas.
-- Voto por participante = posição da contribuição MAIS RECENTE dele.
-- Idempotente: ON CONFLICT DO NOTHING + tópico com id derivado fixo.

BEGIN;

-- Tópico com id determinístico (uuid do debate) — rerun não duplica.
INSERT INTO forum_topic (id, forum_id, author_id, title, body, created_at)
SELECT d.id,
       f.id,
       (SELECT c.author_id FROM debate_contribution c
         WHERE c.debate_id = d.id ORDER BY c.created_at LIMIT 1),
       d.title,
       d.framing,
       d.created_at
  FROM debate d
  JOIN forum f ON f.full_path = 'ministerio-transportes' AND f.org_id = d.org_id
 WHERE EXISTS (SELECT 1 FROM debate_contribution c WHERE c.debate_id = d.id)
ON CONFLICT (id) DO NOTHING;

-- Contribuições viram argumentos (comentários locais com posição), ids preservados.
INSERT INTO forum_topic_comment
       (id, topic_id, author_id, federated, moderation, stance, body, created_at)
SELECT c.id, c.debate_id, c.author_id, false, 'approved',
       CASE c.stance WHEN 'pro' THEN 'favor'
                     WHEN 'con' THEN 'contra'
                     ELSE 'ponderacao' END,
       c.body, c.created_at
  FROM debate_contribution c
  JOIN forum_topic t ON t.id = c.debate_id
ON CONFLICT (id) DO NOTHING;

-- Uma posição por participante: a da contribuição mais recente.
INSERT INTO forum_topic_vote (topic_id, citizen_id, stance, created_at)
SELECT DISTINCT ON (c.debate_id, c.author_id)
       c.debate_id, c.author_id,
       CASE c.stance WHEN 'pro' THEN 'favor'
                     WHEN 'con' THEN 'contra'
                     ELSE 'ponderacao' END,
       c.created_at
  FROM debate_contribution c
  JOIN forum_topic t ON t.id = c.debate_id
 ORDER BY c.debate_id, c.author_id, c.created_at DESC
ON CONFLICT (topic_id, citizen_id) DO NOTHING;

-- Recontagem dos contadores do(s) tópico(s) migrado(s).
UPDATE forum_topic t SET
    favor_count      = v.f,
    contra_count     = v.c,
    ponderacao_count = v.p,
    score            = v.f - v.c,
    comment_count    = cm.total,
    interaction_count = v.votes + cm.locais,
    federated_interaction_count = cm.fede
FROM (SELECT t2.id,
             COUNT(*) FILTER (WHERE vv.stance = 'favor')      AS f,
             COUNT(*) FILTER (WHERE vv.stance = 'contra')     AS c,
             COUNT(*) FILTER (WHERE vv.stance = 'ponderacao') AS p,
             COUNT(vv.citizen_id) AS votes
        FROM forum_topic t2
        LEFT JOIN forum_topic_vote vv ON vv.topic_id = t2.id
       WHERE t2.id IN (SELECT id FROM debate)
       GROUP BY t2.id) v,
     (SELECT t3.id,
             COUNT(cc.id) AS total,
             COUNT(cc.id) FILTER (WHERE NOT cc.federated) AS locais,
             COUNT(cc.id) FILTER (WHERE cc.federated)     AS fede
        FROM forum_topic t3
        LEFT JOIN forum_topic_comment cc
               ON cc.topic_id = t3.id AND cc.moderation = 'approved'
       WHERE t3.id IN (SELECT id FROM debate)
       GROUP BY t3.id) cm
WHERE t.id = v.id AND t.id = cm.id;

COMMIT;

SELECT t.id, t.title, t.favor_count, t.contra_count, t.ponderacao_count,
       t.comment_count, t.interaction_count
  FROM forum_topic t WHERE t.id IN (SELECT id FROM debate);

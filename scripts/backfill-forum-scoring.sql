-- backfill-forum-scoring.sql — recomputa placar (pontos) + karma (ADR-0019, F2). Idempotente
-- (valores ABSOLUTOS recomputados da fonte — rodar 2x não duplica). Rodar 1x em prod após o deploy.

BEGIN;

-- 1. Placar por PONTOS de TODOS os tópicos (mesma fórmula do refresh_topic_counters).
UPDATE forum_topic t SET score =
    COALESCE((SELECT SUM(
        CASE WHEN tv.stance = 'favor' THEN
               CASE WHEN EXISTS (SELECT 1 FROM forum_topic_comment fc2
                      WHERE fc2.topic_id = tv.topic_id AND fc2.author_id = tv.citizen_id
                        AND NOT fc2.federated AND fc2.moderation = 'approved') THEN 2 ELSE 1 END
             ELSE
               CASE WHEN EXISTS (SELECT 1 FROM forum_topic_comment fc2
                      WHERE fc2.topic_id = tv.topic_id AND fc2.author_id = tv.citizen_id
                        AND NOT fc2.federated AND fc2.moderation = 'approved') THEN -2 ELSE -1 END
        END)
      FROM forum_topic_vote tv WHERE tv.topic_id = t.id), 0)
  + COALESCE((SELECT SUM(
        CASE
          WHEN fc.stance = 'favor'  AND fcv.stance = 'favor'  THEN 2
          WHEN fc.stance = 'favor'  AND fcv.stance = 'contra' THEN -1
          WHEN fc.stance = 'contra' AND fcv.stance = 'favor'  THEN -2
          WHEN fc.stance = 'contra' AND fcv.stance = 'contra' THEN 1
          ELSE 0 END)
      FROM forum_comment_vote fcv
      JOIN forum_topic_comment fc ON fc.id = fcv.comment_id
     WHERE fc.topic_id = t.id AND fc.moderation = 'approved' AND NOT fc.federated), 0);

-- 2. Karma dos autores de argumentos, a partir dos votos que seus comentários receberam de OUTROS
--    (SO: favor=+10, contra=-2; sem self-vote). Absoluto → idempotente.
UPDATE citizen c SET karma = COALESCE((
    SELECT SUM(CASE fcv.stance WHEN 'favor' THEN 10 WHEN 'contra' THEN -2 ELSE 0 END)
      FROM forum_comment_vote fcv
      JOIN forum_topic_comment fc ON fc.id = fcv.comment_id
     WHERE fc.author_id = c.id AND fcv.citizen_id <> c.id
), 0)
WHERE EXISTS (SELECT 1 FROM forum_topic_comment fc2 WHERE fc2.author_id = c.id);

COMMIT;

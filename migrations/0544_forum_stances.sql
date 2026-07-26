-- 0544_forum_stances — fusão DEBATE→FÓRUM (issue #19, 2026-07-26).
--
-- A dualidade Debates × Fóruns acaba: o tópico de fórum adota o modelo
-- funcional consolidado do debate — a participação é uma POSIÇÃO
-- ('favor' | 'contra' | 'ponderacao'), uma por cidadão (pode mudar), e o
-- comentário pode carregar a posição do autor no momento do argumento.
-- Contadores por posição ficam materializados no tópico (exibição em lista).
--
-- score continua = favor - contra (ordenação "em alta" preservada);
-- interações contáveis continuam votos + comentários LOCAIS (patamares).
--
-- Idempotente: rerun-safe.

BEGIN;

-- Voto ±1 vira posição. Backfill: +1 → favor, -1 → contra.
ALTER TABLE forum_topic_vote ADD COLUMN IF NOT EXISTS stance text;
UPDATE forum_topic_vote
   SET stance = CASE WHEN value = 1 THEN 'favor' ELSE 'contra' END
 WHERE stance IS NULL;
ALTER TABLE forum_topic_vote ALTER COLUMN stance SET NOT NULL;
ALTER TABLE forum_topic_vote
    DROP CONSTRAINT IF EXISTS forum_topic_vote_stance_check;
ALTER TABLE forum_topic_vote
    ADD CONSTRAINT forum_topic_vote_stance_check
    CHECK (stance IN ('favor', 'contra', 'ponderacao'));
ALTER TABLE forum_topic_vote DROP COLUMN IF EXISTS value;

-- Comentário pode carregar a posição do autor (NULL = sem posição declarada;
-- comentários federados nunca têm posição — não votam).
ALTER TABLE forum_topic_comment ADD COLUMN IF NOT EXISTS stance text;
ALTER TABLE forum_topic_comment
    DROP CONSTRAINT IF EXISTS forum_topic_comment_stance_check;
ALTER TABLE forum_topic_comment
    ADD CONSTRAINT forum_topic_comment_stance_check
    CHECK (stance IS NULL OR stance IN ('favor', 'contra', 'ponderacao'));

-- Contadores por posição, materializados (recalculados sob o row lock do tópico).
ALTER TABLE forum_topic
    ADD COLUMN IF NOT EXISTS favor_count      bigint NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS contra_count     bigint NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS ponderacao_count bigint NOT NULL DEFAULT 0;

-- Backfill dos contadores + score a partir dos votos existentes.
UPDATE forum_topic t
   SET favor_count      = s.f,
       contra_count     = s.c,
       ponderacao_count = s.p,
       score            = s.f - s.c
  FROM (SELECT topic_id,
               COUNT(*) FILTER (WHERE stance = 'favor')      AS f,
               COUNT(*) FILTER (WHERE stance = 'contra')     AS c,
               COUNT(*) FILTER (WHERE stance = 'ponderacao') AS p
          FROM forum_topic_vote GROUP BY topic_id) s
 WHERE s.topic_id = t.id;

COMMENT ON COLUMN forum_topic_vote.stance IS
    '0544: posição do cidadão — favor | contra | ponderacao (fusão debates→fóruns).';
COMMENT ON COLUMN forum_topic_comment.stance IS
    '0544: posição declarada junto do argumento (NULL = sem posição / federado).';

COMMIT;

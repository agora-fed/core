-- 0672_socrates_idea_body — SOCRATES v3: o espelho passa a carregar a IDEIA
-- INTEIRA (pauta + apoios + situação), e não só o título.
--
-- O que estava quebrado: o tópico espelhado tinha só o título da ideia. O
-- cidadão chegava no fórum sem a proposta em si — não havia o que debater. E o
-- número de apoios, que o sweep re-sincronizava em `apoiamentos`, ficava
-- INVISÍVEL: o corpo do tópico era escrito uma única vez na criação e nunca
-- reescrito, então o banco atualizava e o fórum continuava mostrando o número
-- do dia do espelhamento (quando mostrava — 6 dos 11 espelhos têm `apoiamentos`
-- NULL porque vieram da fonte HTML, que só dá ids).
--
-- A correção usa o endpoint JSON público POR IDEIA do e-Cidadania
-- (`restideialegislativa?id=<ID>`), que devolve a descrição integral, o
-- contador de apoios como INTEIRO puro e a situação institucional da ideia.
-- Daí as colunas novas:
--
--   * `descricao`       — a PAUTA: o texto integral da proposta, o que faltava
--                         no corpo do tópico. Guardado pra que o refresh saiba
--                         se mudou sem reescrever o tópico à toa;
--   * `situacao`        — a situação institucional ("Convertida em Proposição",
--                         "Aguardando envio à CDH", …), o dado que diz se a
--                         ideia ainda está viva no Senado;
--   * `apoiamentos_num` — o contador como NÚMERO. A coluna `apoiamentos` (text)
--                         continua existindo por compatibilidade: ela guarda a
--                         formatação do Senado ("20.771", com ponto de milhar),
--                         que não dá pra comparar nem ordenar. O endpoint por
--                         ideia dá o inteiro, então aqui ele fica inteiro;
--   * `body_synced_at`  — quando o CORPO do tópico foi reescrito pela última vez
--                         com esses dados. NULL = o tópico ainda tem o corpo
--                         antigo (só título): é exatamente esse o critério que o
--                         backfill usa pra saber quem precisa ser preenchido.
--
-- OWNER: ALTER TABLE socrates_mirror OWNER TO dsoc
--
-- Idempotente: rerun-safe (ADD COLUMN IF NOT EXISTS).

BEGIN;

ALTER TABLE socrates_mirror ADD COLUMN IF NOT EXISTS descricao       text;
ALTER TABLE socrates_mirror ADD COLUMN IF NOT EXISTS situacao        text;
ALTER TABLE socrates_mirror ADD COLUMN IF NOT EXISTS apoiamentos_num bigint;
ALTER TABLE socrates_mirror ADD COLUMN IF NOT EXISTS body_synced_at  timestamptz;

-- O refresh do sweep prioriza quem está mais desatualizado (NULLS FIRST = os
-- espelhos que nunca tiveram o corpo preenchido vêm na frente).
CREATE INDEX IF NOT EXISTS socrates_mirror_body_synced_idx
    ON socrates_mirror (body_synced_at NULLS FIRST);

ALTER TABLE socrates_mirror OWNER TO dsoc;

COMMIT;

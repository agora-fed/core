# ADR-0019 — Placar de deliberação por pontos + karma do usuário (estilo StackOverflow/Odoo)

- **Status:** Proposed (aguardando aprovação do Marcos)
- **Context:** Diretriz do Marcos (2026-07-28). O placar atual do fórum conta 3 posições
  (favor/contra/ponderação) como contadores de VOTOS, o que gera incoerência visual (um argumento
  "Contra" sob um "Contra 0" — ver caso `d1111111…`). Trocar por um **placar com sinal** (estilo SO/
  Odoo Forum) onde argumentar pesa mais que só votar, e votos nos argumentos **também dão KARMA** ao
  autor. **Ponderação é eliminada.** Implementamos o CONCEITO (regras não são propriedade de ninguém),
  não copiamos código do Odoo (evita amarração de licença LGPL).

## Modelo de pontuação

São **dois números distintos**, computados da MESMA fonte (votos de tópico + votos de argumento):

### 1. Placar do TÓPICO (com sinal — "qual lado está ganhando")
`+` = apoio à proposta do tópico, `−` = rejeição.

**Posição da pessoa** (UMA por cidadão — o voto de tópico é único; argumentar é o dobro):
| Situação | Pontos |
|---|---|
| A favor + comentou (≥1 argumento) | **+2** |
| A favor, só votou | **+1** |
| Contra + comentou | **−2** |
| Contra, só votou | **−1** |

**Voto NO argumento** (cada voto soma; `favor`-vote = positivo, `contra`-vote = negativo):
| Argumento | Voto positivo (favor) | Voto negativo (contra) |
|---|---|---|
| Comentário **a favor** | **+2** | **−1** |
| Comentário **contra** | **−2** | **+1** |

`forum_topic.score` passa a ser essa soma com sinal. (Hoje era `favor_count − contra_count`.)

### 2. KARMA do USUÁRIO (reputação — estilo SO, independe do lado)
Recompensa QUALIDADE do argumento, não o lado. Por voto recebido num comentário do usuário:
- voto **positivo** (favor) no meu comentário → autor **+10 karma**
- voto **negativo** (contra) no meu comentário → autor **−2 karma**

Nova coluna `citizen.karma int NOT NULL DEFAULT 0`. Exibida como selo no perfil/autoria (estilo SO).

### Exemplo (o tópico `d1111111…` após a mudança)
3 votos favor; 1 deles comentou. Comentários hoje: 2 favor, 1 contra (ponderação apagada). Sem votos
em comentários ainda. Placar = (favor+comentou +2) + (favor só-voto +1) + (favor só-voto +1) = **+4**.
Coerente: o tópico está "ganhando a favor", e o argumento contra existe mas ninguém o endossou.

## Decisões de schema (migration 0665)

1. **Ponderação eliminada:** `CHECK (stance IN ('favor','contra'))` em `forum_topic_vote`,
   `forum_topic_comment`, `forum_comment_vote`. **APAGAR** linhas `stance='ponderacao'` (voto e
   comentário) — decisão do Marcos. Dropar colunas `ponderacao_count` de `forum_topic` e
   `forum_topic_comment`.
2. **Karma:** `ALTER TABLE citizen ADD COLUMN karma int NOT NULL DEFAULT 0`.
3. `forum_topic.score` re-significado (pontos com sinal) — sem mudança de tipo.

## Mudanças de código

- **`domain.rs`:** `enum Stance { Favor, Contra }` (remove `Ponderacao`); `parse_input` rejeita
  ponderacao; atualizar testes.
- **`queries.rs`:** reescrever o recount do tópico (hoje `favor_count − contra_count`) para a fórmula
  de pontos acima (base por pessoa via `forum_topic_vote` + comentou? via EXISTS em
  `forum_topic_comment`; amplificação via `forum_comment_vote` × stance do comentário). Regenerar `.sqlx`.
- **Karma:** ao votar num comentário (`vote_comment`), aplicar delta de karma ao AUTOR do comentário
  (+10/−2); ao remover/trocar voto, reverter. `citizen.karma` acumulado.
- **`service.rs`/`http.rs`:** remover ponderacao dos caminhos; manter a semântica "comentar com
  posição = seu voto".
- **Frontend (`ForumsApp.svelte`, `api.ts`, `debates/index.astro`):** remover a coluna/botão
  Ponderação (2 colunas: A favor / Contra); exibir **placar com sinal** no topo; selo de **karma** na
  autoria (estilo SO).

## Impacto no loop de consequência (DECIDIDO — o placar vira o gatilho)
O encaminhamento ao gabinete passa a disparar pelo **placar de PONTOS**, não mais por volume de
interação: **placar líquido ≥ 10 pontos → e-mail pro gabinete** (com o relógio de resposta / SLA
existente). Patamar **fixo em 10** por ora (decisão do Marcos, 2026-07-28); o modelo proporcional-ao-
eleitorado (`threshold_policy.rs`) fica como fase futura, medido em pontos.

Racional: um tópico controverso (muito engajamento, placar líquido ~0) **não** escala; um claramente
apoiado (líquido ≥ 10) escala. Melhor sinal que volume bruto. **Só escala com apoio líquido positivo.**

Implementação: em `service.rs::after_interaction`, trocar o gatilho de `interaction_count` cruzando
`forum.thresholds` por `score >= 10` cruzando (uma vez — guarda em `next_threshold_idx`/flag pra não
reenviar). O disparo do e-mail/consequência (`consequence_sla`/`consequence_response`) é reusado.

## Migração de dados (backfill)
1. Apagar linhas ponderacao (voto + comentário + votos nesses comentários).
2. Recomputar `score` de TODOS os tópicos com a fórmula nova.
3. Backfill de `citizen.karma` a partir dos `forum_comment_vote` existentes (+10/−2 por voto).

## Fases
- **F1 — schema + domínio:** migration 0665 (ponderação out, karma in), `Stance` enum, `.sqlx`. Build verde.
- **F2 — placar + karma no backend:** recount novo + accrual de karma + backfill. Testes de fórmula.
- **F3 — frontend:** 2 colunas, placar com sinal, selo de karma.
- **F4 — deploy + verificação** (recompute em prod, conferir o tópico exemplo).

## Consequências
- Placar mais legível e "gamificado" (incentiva argumentar, não só votar).
- Karma cria reputação — base pra futuros privilégios (moderação, peso) estilo SO. (Fora de escopo agora.)
- Ponderação some do produto — argumentos neutros deixam de existir como categoria.

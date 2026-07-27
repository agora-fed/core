# ADR-0018 — Atividade legislativa + destilação para meta-análise cívica

- **Status:** Accepted (fundação); destilação NLP/LLM = fase seguinte
- **Context:** North-star do Marcos (2026-07-27, issue #73). Depende do pipeline de extração por
  plataforma (#72, [ADR-0017](ADR-0017-civic-data-extraction.md)). Objetivo: mapear com **cobertura
  máxima** a atividade dos representantes em todas as esferas (vereador → deputado → senador →
  executivo), por partido/região/tempo, e **destilar** isso em informação navegável para o cidadão —
  "o que meus representantes realmente fazem e para onde o país vai" — como **sinal cívico agregado**,
  não opinião editorial.

## Superfície de dados confirmada (SAPL, verificada ao vivo 2026-07-27)

A mesma API SAPL que dá contatos (#72) expõe a atividade legislativa — estruturada e paginada:

| Endpoint | Conteúdo | Campos-chave |
|---|---|---|
| `/api/materia/materialegislativa` | proposições | `numero, ano, data_apresentacao, ementa`, autoria (via `/materia/autoria`) |
| `/api/materia/tramitacao` | tramitações | `data_tramitacao, status, texto, turno, urgente` |
| `/api/sessao/ordemdia` | pauta + votações | `data_ordem, resultado, tipo_votacao, votacao_aberta, turno` |
| `/api/sessao/sessaoplenaria` | sessões (base das atas) | data, tipo, legislatura |
| `/api/norma/normajuridica` | normas aprovadas | `texto_integral, ementa, indexacao, data_publicacao` |

Ou seja: **proposições/autoria, votações/resultados, presença (via mandato/sessão) e texto integral**
(ementa + norma) são todos extraíveis pela API padronizada — sem scraper por município, coerente com
a estratégia por-plataforma do #72. Federal/estadual têm APIs próprias já em uso (Câmara/Senado) +
`parlamentar_activity.rs`.

## Decision

Pipeline de 3 estágios, reaproveitando o toolchain `scripts/civic/` + `civic_source` (#72):

### 1. Extração de atividade (fundação — mesma infra do #72)
Estender o cliente SAPL com coletores de `materialegislativa`, `ordemdia` (votações), `tramitacao` e
`normajuridica`, chaveados por `civic_source`. Persistir num schema de atividade
(`civic_activity_*`: proposição, votação, presença) ligado a `mandate` pelo mesmo casamento
nome+partido+município do extrator de contatos. Fonte primária pública (atos públicos).

### 2. Destilação (fase NLP/LLM — próxima)
Pipeline reprodutível sobre ementas + texto integral + histórico de votação → temas, posições,
promessas, e **coerência voto × discurso**. Comparável por partido / UF / esfera / período. Toda
inferência **citando a fonte primária** (link à matéria/norma/sessão) — reprodutível e auditável,
nunca opinião editorial.

### 3. Entrega ao cidadão
Meta-análise navegável ligada ao **scorecard/loop de consequência** existente: o que dizem × o que
votam × o que cumprem (SLA de resposta às demandas). Sinal cívico agregado.

## Rationale

A infraestrutura de extração por-plataforma (#72) já resolve o "de onde vêm os dados". A atividade
está na MESMA API — então o custo marginal de cobrir votações/proposições/atas é o coletor, não uma
nova estratégia. A destilação é onde entra julgamento de produto (o que agregar, como não editorializar);
por isso é fase separada, começando por um recorte (uma esfera/UF) antes de generalizar.

## Consequences

- Reusa `civic_source` + o casamento com `mandate` do #72 — nada de scraper por município.
- Volume real: `tramitacao` e `materialegislativa` têm centenas de milhares de linhas **por instância** —
  a extração precisa ser incremental (por data) e priorizada por população/esfera; **candidato natural a
  workflow multi-agente** com fan-out por fonte, custo estimado antes de rodar.
- Conformidade: só ato público, com citação à fonte; destilação transparente (prompt/versão registrados).
- A fase NLP/LLM depende de decisão de produto sobre escopo do 1º recorte e forma de apresentação.

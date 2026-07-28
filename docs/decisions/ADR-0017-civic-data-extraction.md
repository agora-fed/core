# ADR-0017 — Extração de dados cívicos: por plataforma, não por município; TSE como backbone

- **Status:** Accepted
- **Context:** Diretriz do Marcos (2026-07-27) — popular/enriquecer os ~70k mandatos com contatos
  (nome, e-mail, telefone institucionais) sem manter um scraper por município. Ver épico em
  git.pop.coop/brasil/democracia-social (issue de extração).

## Decision

Enriquecer a base de **mandatos** (não confundir com a base própria de campanha, `campaign_contact`
do F4/#61 — pool distinto) por um pipeline de 4 camadas:

### 1. Roster canônico — TSE dados abertos
`dadosabertos.tse.jus.br` / DivulgaCandContas dão o roster de **todos os eleitos** (nome, cargo,
partido, município, número). É a fonte da verdade de *quem tem mandato onde*. Atualiza **por
eleição** (4 anos, escalonado: municipal 2024/2028, geral 2026/2030) — **não** ano a ano.

### 2. Federal/estadual por API oficial
Câmara (`dadosabertos.camara.leg.br`) + Senado (`legis.senado.leg.br`) já dão e-mail institucional
estruturado (já em uso). 27 Assembleias Legislativas = 27 portais (não milhares).

### 3. Municipal: extrair POR PLATAFORMA, não por município
As ~5.570 câmaras rodam um punhado de plataformas. A maior é o **Portal Modelo / SAPL do Interlegis**
(programa gratuito do Senado): **1.200+ câmaras** com a MESMA estrutura, e o SAPL tem **API**. Somados
os vendors privados (`camaraonline.org`, IPM, Câmara Sem Papel…), **~5-10 extratores cobrem a maioria**.
Fluxo: *fingerprint* de cada câmara → agrupa por plataforma → **um extrator por plataforma**.

### 4. Casamento + ingestão
Casar (nome + partido + município, fuzzy) com o roster do TSE → dedupe → alimentar o pipeline que já
existe (`scripts/seed-*.py`, módulo `politico_contacts`).

**Priorização por população** (grandes municípios concentram a maior parte da população em poucos %
das câmaras): SP → PR → SC → RS → RJ → ES → MG → GO → MT → MS → BA → Nordeste.

## Rationale

Manter 5.570 scrapers é insustentável e frágil. Agrupar por software colapsa o problema em ~poucos
extratores; o roster do TSE dá a espinha dorsal estável e evita depender de scraping para saber *quem*
existe. A cadência real é eleitoral (4 anos), então o custo de manutenção é baixo entre pleitos.

## Consequences

- Conformidade: usar SÓ contato **institucional** (gabinete/câmara) — dado público de transparência;
  nunca telefone/e-mail pessoal. Respeitar robots.txt/ToS. Accountability, não marketing.
- Trabalho: (a) baixar/normalizar roster TSE; (b) mapa município→plataforma (fingerprint); (c)
  extratores por plataforma (Interlegis/Portal Modelo + camaraonline.org primeiro); (d) casar + seed.
- Ortogonal ao F4 (base própria de campanha) — este pipeline enriquece os MANDATOS da plataforma.
- Candidato a um workflow multi-agente (fan-out por município/UF, priorizado por população).

## Status de implementação (2026-07-27)

Camada 3 (municipal por plataforma) **entregue e provada** para SAPL/Interlegis:

- `migrations/0662_civic_source.sql` — catálogo município→plataforma+URL (aplicado local + prod).
- `scripts/civic/sapl_client.py` — roster VIGENTE via `mandato/legislatura` + partido via `filiacao`;
  paginação cobre os dois formatos SAPL (`pagination` e DRF).
- `scripts/civic/fingerprint_sapl.py` — probe concorrente da convenção `sapl.<slug>.<uf>.leg.br` →
  grava `civic_source`.
- `scripts/civic/extract_sapl.py` — casa nome (fuzzy) + partido com `mandate`; **só e-mail
  institucional** (transparência); enriquece `mandate.public_email` **apenas quando placeholder**;
  **dry-run por padrão**, `--apply` grava.
- `scripts/civic/confirmed_sapl_seed.sql` — 25 câmaras SAPL verificadas ao vivo (SP PR SC RS RJ ES MG).

Prova end-to-end (slice de 25 câmaras × mandatos reais de prod): 26 câmaras respondendo, **499
vereadores vigentes, 313 casados, 45 mandatos enriquecíveis** — matches conferidos manualmente.
A cobertura por-e-mail depende da qualidade da fonte (capitais rodam portais próprios; municípios
pequenos usam e-mail pessoal, que é descartado). Camadas 1/2/4 e o fan-out nacional seguem pendentes
(workflow multi-agente com custo estimado). Atividade legislativa (atas/votações) → [ADR-0018](ADR-0018-legislative-activity-distillation.md).

### Outras plataformas municipais (2026-07-27)

Investigadas ao vivo as 3 plataformas não-SAPL do épico #72 (camaraonline, IPM, Câmara Sem Papel):

- **camaraonline** — ENTREGUE e provado. Vendor privado, sem API: dados no HTML público. Assinatura
  de fingerprint = link `camaraonline.org/cm_<slug>` no HTML; listagem em `/vereadores`; detalhe em
  `/vereador/<id>/<slug>` (template moderno) ou `/vereadores/<id>/biografia` (legado). Convenção de
  host `camara<slug>.<uf>.gov.br`. `scripts/civic/{camaraonline_client,fingerprint_camaraonline,
  extract_camaraonline}.py` + `confirmed_camaraonline_seed.sql`. Reusa o matcher do `extract_sapl`.
  Prova (Santana de Parnaíba/SP, 2026-07-27): 17 vereadores vigentes, 16 com e-mail institucional
  `@camarasantanadeparnaiba.sp.gov.br`; casou 7/7 fixtures, enriqueceu 6 (o 7º só tinha gmail →
  descartado; idempotente no rerun). O template legado (ex.: Caieiras) ofusca e-mail via Cloudflare
  email-protection — NÃO decodificamos (sinal anti-scraping; ToS) → roster sim, e-mail não.

- **IPM (Atende.net)** — NÃO extraível de forma limpa. Fingerprint confiável (`<slug>.atende.net`,
  título "Portal do Cidadão", fragmentos `origem=".../static/portal/html/elementos/"`). A rota
  `/cidadao/pagina/vereadores` existe, mas o conteúdo dos vereadores é renderizado por JS/AJAX
  (componentes dinâmicos), ausente do HTML servido; os endpoints dinâmicos (`?rot=…`) são
  **Disallow** no robots.txt. Sem motor JS e respeitando o robots, não há dado estruturado acessível;
  e nenhum e-mail institucional aparece no HTML. Não foi construído extrator (evitar esboço quebrado).

- **Câmara Sem Papel** — NÃO é uma plataforma única. É um conceito ("legislativo sem papel")
  implementado por vários fornecedores distintos (1doc, Ágape, NOPAPER Cloud, SPL/ASP.NET…), sem
  fingerprint nem endpoint uniforme. É um sistema de PROCESSO/documento legislativo (proposições,
  assinatura digital), adjacente ao ADR-0018, não um diretório de contatos. A instância probada
  (Caçapava/SP, `spl/parlamentares.aspx`) lista nomes de parlamentares mas **não publica e-mail**.
  Não há alvo de extração de contato; nenhum extrator construído.

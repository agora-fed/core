# PINDORAMA / DemocraciaBR — Plano Estratégico de Melhorias Heurísticas

*Análise de 09/07/2026 — 5 semanas antes da abertura do registro de candidaturas no TSE (15/08/2026), ~12 semanas antes do 1º turno.*

> Documento de produto (conteúdo cívico → português, por convenção do repo).
> Norte de engenharia continua sendo `PLAN.md`; roadmap tático em `docs/ROADMAP.md`.

---

## 1. Diagnóstico honesto

### 1.1 Onde a plataforma é genuinamente forte

- **O motor de consequência é real, não vitrine.** `crates/components/consequence/src/service.rs`: SLA idempotente por `(proposal, cluster)`, outbox transacional (ADR-0006), outcome write-once ("o silêncio público é permanente"), sweep determinístico com clock injetado, guarda TOCTOU contra corrida resposta×expiração.
- **O loop está tecnicamente fechado ponta a ponta.** `ProposalCreated → ModerationEvaluateSub → publish → consensus.ingest → votes → threshold_crossed → start_sla → notify (gabinete por e-mail + autor in-app/push) → respond/sweep → scorecard → auto-federação da Note ActivityPub` (0.26.24-fase-E). O worker sobe com 14 subscriptions.
- **Dados reais de poder.** ~70k mandatos (Câmara + Senado + vereadores + prefeitos), com e-mail público, foto, partido, UF. `parlamentar_activity.rs` e o dashboard CEAP/CEAPS já consomem dados abertos oficiais.
- **Federação como canal de distribuição.** AP S2S completo (webfinger, nodeinfo, HTTP signatures), Mastodon client API + OAuth — apps Mastodon existentes logam na plataforma. Auto-federação de thresholds em nome do autor é alavanca de alcance que nenhum civic-tech tem.
- **Higiene institucional acima da média:** LGPD export/delete real, páginas institucionais editoriais, stats públicas sem PII, admin console completo, rate-limits publicados, AGPLv3 + Contrato Social.

### 1.2 Onde é teatro / vitrine (honestidade brutal)

| Área | O que aparenta | O que é de fato |
|---|---|---|
| **Consenso semântico** | "pgvector clusteriza propostas em sinal genuíno" | `StubEmbedder` (`consensus/src/domain.rs`) — feature hashing FNV-1a, não embedding semântico. "Creche no bairro" e "vaga em berçário" nunca clusterizam. |
| **Threshold** | "consenso cruzou o limiar" | O **autor escolhe** o threshold (qualquer inteiro positivo). Threshold=1 sobre mandato de 200k eleitores dispara SLA. Sinal gameável. |
| **Silêncio público** | "o político ignorou" | 1 e-mail SMTP pro `mandate.public_email`, sem recibo verificável, sem reenvio, sem prova pública de entrega. *"Nunca recebi"* derruba a narrativa. |
| **Lado do político** | "respostas públicas no placar" | Responder exige aceitar convite + usar painel. Com ~0 gabinetes onboardados, o placar tende a 100% silêncio — ruído, não sinal. |
| **Scorecard de promessas** | "promessas vs. entrega" | `record_promise`/`mark_promise_delivered` existem, mas sem pipeline de dados nem UI — metade "promessas" vazia. |
| **Eleições 2026** | "comparador de candidatos" | 10 candidacies seed de exemplo. Correto até 15/08, mas hoje é maquete com countdown. |
| **Identidade** | "título de eleitor validado" | Validação algorítmica de dígitos; gov.br dormant. Sybil barato onde importa. |
| **Mobile-first** | princípio FROZEN do PLAN | Web PWA com push (bom!), Flutter sem evidência de entrega, WhatsApp é enum sem transporte. |
| **Espaços Tier-2** | assemblies, initiatives, consultations… | Skeletons. OK pelo anti-bloat, mas não fingir que existem. |
| **Multi-tenancy** | "sem ponto único de takedown" | `DEFAULT_ORG_UUID` hardcoded — na prática single-tenant. |

### 1.3 Síntese

O loop está **fechado em software e aberto em sociologia**. Os dois elos fracos: (a) o *sinal* precisa ser estatisticamente legítimo (embeddings reais + threshold proporcional), e (b) a *consequência* precisa ser à prova de negação (prova de notificação) e ter caminho de resposta com atrito zero pro gabinete. Tudo o mais é amplificação.

---

## 2. Melhorias rankeadas (impacto democrático × viabilidade)

Escala: impacto ★1–5, viabilidade ◆1–5 (5 = fatia deployável em dias).

1. **Embeddings reais locais no `consensus`** — ★5 ◆5. Substituir `StubEmbedder` por modelo local multilíngue (bge-m3 / multilingual-e5-small via ONNX). O trait `Embedder` foi desenhado pra isso. Re-cluster idempotente do backlog; cluster com SLA é imutável. *A melhoria de maior alavancagem por linha de código do repositório.*
2. **Prova de notificação ("AR digital do silêncio")** — ★5 ◆4. Reenvio escalonado (D0/D+1/D+2) com recibo SMTP persistido (`notification_receipt` hash-encadeada), timeline pública no SLA, cadeia embutida na Note federada do silêncio. Muda "silêncio" de acusação para **fato auditável**.
3. **Reply-to-respond (responder por e-mail, sem conta)** — ★5 ◆4. Token HMAC por SLA no e-mail ao gabinete; a resposta do e-mail vira `ConsequenceService::respond` publicada (double-opt). O gargalo real do loop é adoção do político; atrito zero resolve.
4. **Threshold dinâmico proporcional ao eleitorado** — ★4 ◆5. Fração do eleitorado TSE do território, com piso/teto; autor não escolhe mais. Legitimidade estatística do gatilho.
5. **Pipeline TSE DivulgaCandContas pronto para 15/08** — ★5 ◆4. Ingestão automatizada dos CSVs de candidaturas → `elections`/`candidacies`; ensaiar HOJE com dataset 2022.
6. **"O placar segue o candidato"** — ★5 ◆4. Candidacy × mandate: todo candidato ex-mandatário exibe o placar do mandato anterior no comparador. Silêncio vira **custo eleitoral**. Matching conservador + contestação (risco jurídico gerenciado).
7. **OG cards dinâmicas + placar embedável** — ★4 ◆5. PNG por mandato/SLA + widget iframe/JSON pra imprensa. O placar só gera consequência se circula fora.
8. **Votações nominais persistidas × demandas** — ★4 ◆4. `roll_call_vote` + cruzamento semântico (via item 1): "73% dos apoiadores pediram X; seu deputado votou contra".
9. **WhatsApp de verdade** — ★5 ◆2-3. Transporte real pro `ChannelKind::WhatsApp`. O Brasil mora no WhatsApp. Iniciar burocracia Meta JÁ. Canal de alcance, nunca de armazenamento (ADR).
10. **Mapa de opinião estilo Pol.is nos debates** — ★4 ◆3. Micro-afirmações votáveis, clustering de matrizes de voto, afirmações-ponte viram propostas. Pol.is encontra consenso; aqui ele entra direto no motor de SLA.
11. **Verificação graduada com peso público de sinal** — ★3 ◆4. Composição do apoio por nível (e-mail < título < gov.br bronze/prata/ouro) exibida publicamente; terminar JWKS do gov.br.
12. **Digest público mensal por território** — ★3 ◆5. E-mail + Note + RSS: "seus políticos este mês: X responderam, Y silenciaram, Z gastaram".
13. **Multi-tenancy real** — ★4 ◆2. Remover `DEFAULT_ORG_UUID`; instância por município. **Depois da eleição.**
14. **Promessas de campanha estruturadas** — ★4 ◆3. Planos de governo (PDFs TSE) → promessas no scorecard → ciclo de cobrança 2027-2030.
15. **PWA instalável agressiva + TWA Play Store** — ★3 ◆5. 70% do mobile-first em 1 semana, sem esperar Flutter.

---

## 3. Apostas assimétricas (ninguém no mundo faz)

### A — "Cartório criptográfico do silêncio"
Log append-only Merkle (estilo Certificate Transparency) de todo evento do ciclo de consequência, raiz publicada como Note federada assinada, espelhável por universidades/imprensa. Converte a plataforma de "site que acusa" em **infraestrutura notarial** — a defesa definitiva contra a contestação judicial/política que virá se funcionar. Base: outbox ADR-0006 + roadmap Fase 3 já prevê audit log. Novo crate `platform/notary`.

### B — Cada candidato 2026 nasce como ator federado com passado
No registro TSE, cada candidatura vira actor ActivityPub (`@nome-cargo-uf@democracia.social.br`) que 1M+ usuários do fediverso podem seguir — feed automático: placar herdado, gastos CEAP, votações, respostas/silêncios em campanha. A plataforma vira **a camada de dados da eleição no fediverso**, sem ninguém criar conta nela. Cuidado Lei 9.504 (só fatos de fonte oficial, direito de resposta). Começar com ex-mandatários (~2k actors).

### C — IA soberana como "escrivã pública" + detector de resposta evasiva
Modelo local com 3 ofícios auditáveis: (1) redige a demanda canônica de cada cluster; (2) redige a carta oficial ao gabinete; (3) classifica publicamente se a resposta endereça o mérito (`respondeu / parcial / evasiva`) com prompt+modelo+justificativa publicados e apelação humana via `moderation`. Cria a métrica que mata o teatro do "agradecemos seu contato": taxa de resposta **de mérito**. Começar como rótulo comunitário assistido.

---

## 4. Sequência recomendada (calendário eleitoral no centro)

**Restrição dura: registro de candidaturas abre 15/08/2026.**

- **Semanas 1–2 (até ~24/07):** #1 embeddings reais (env flag → re-cluster), #4 threshold dinâmico, #2 prova de notificação. Iniciar burocracias de longo lead-time: Meta/WhatsApp (#9), gov.br client_id (#11), parceria de espelhamento (Aposta A).
- **Semanas 3–4 (até ~07/08):** #3 reply-to-respond + onboarding dos 594 gabinetes federais (alvo honesto: 20-30 respondendo muda a narrativa), #7 OG cards + embed, #5 pipeline TSE ensaiado com 2022.
- **Semana 5 (11–15/08):** ingestão real DivulgaCandContas (#5) + #6 placar-segue-candidato no ar na mesma semana + Aposta B fatia 1 (actors de ex-mandatários).
- **Semanas 6–8:** #8 votações nominais, #12 digest mensal, #14 planos de governo, Aposta A fatia 1, #15 PWA/TWA.
- **Adiar para pós-outubro:** #13 multi-tenancy, #10 Pol.is-debates, Flutter nativo, Aposta C automatizada.

**Riscos transversais (documentar em ADRs):** Lei 9.504/97 (política editorial "só fatos de fonte oficial + direito de resposta"); neutralidade partidária (publicar metodologia de cobertura em /transparencia); LGPD sobre dados de candidatos (base legal documentada).

## Arquivos críticos para implementação

- `crates/platform/consensus/src/domain.rs` — trait `Embedder`/`StubEmbedder` (#1)
- `crates/components/proposals/src/domain.rs` — política de threshold (#4)
- `crates/components/consequence/src/service.rs` — motor de SLA (#2, #3)
- `crates/gateway/src/elections.rs` — comparador 2026 (#5, #6)
- `crates/gateway/src/proposal_delivery.rs` — entrega ao gabinete (#2, #3)

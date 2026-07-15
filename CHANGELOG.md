# Changelog

All notable changes to this project are documented here. Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
the project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Per PLAN.md principle 1, we **credit Decidim concepts we port**.

## [Unreleased]

### Added
- **0.31.0-doacoes — o serviço de doações/financiamento vira produto** (aprova o
  protótipo 0.30.7): migration `0523` cria `campaign_finance_entry` (declaração
  **append-only** — lançamento não se edita, revoga-se com `revoked_at` e
  relança-se) e `campaign_fundraising_config` (meta, conta de campanha, link de
  financiamento coletivo homologado, publicação). Novo `gateway/src/campanha.rs`:
  `GET /me/campanha`, `POST/DELETE /me/campanha/lancamentos[/{id}]`,
  `PUT /me/campanha/config` — **exclusivo de conta "tipo político"** (vínculo em
  `mandate_identity_binding`, o mesmo do painel-mandato): leitura devolve
  `is_politico=false` para os demais, escrita responde 403; 3 testes novos no
  http_surface (401 anônimo, 403 não-político, roundtrip completo). Front:
  `/servicos/painel` liga o painel na API (lançar, revogar com confirmação,
  config com meta em reais → centavos) e o **submenu do perfil (AuthMenu) ganha
  "💰 Doações e financiamento"** — visível só para político, cache
  `dsoc_is_politico` no padrão do `dsoc_is_admin`, limpo no logout. A ilha
  protótipo (`DoacoesPainelPrototipo`) sai do repo.
- **0.30.7-doacoes-prototipo — protótipo navegável do painel de doações**: nova
  `/servicos/painel` (noindex, fora do menu) monta a ilha `DoacoesPainelPrototipo`:
  logada, a pessoa se vê na interface (perfil real via `GET /api/v1/me`) com dados
  de exemplo em 4 abas — Visão geral (arrecadado × meta, gastos, doações, selo),
  Financiamento (entradas/saídas + form local de lançamento), Doações (recibos,
  CPF mascarado) e Configurações. Baner deixa explícito: demonstração, nada é
  gravado. Serve pra aprovar a UX antes de construir o backend do serviço.
- **0.30.6-servicos — página institucional dos Serviços para candidaturas**: nova
  `/servicos` apresenta os dois serviços para quem disputa mandato — **declarar o
  financiamento de campanha** (página pública com histórico, complementar — nunca
  substituta — à prestação de contas oficial, com link pro DivulgaCandContas) e
  **pedir doações** (divulgação dentro das regras da Lei 9.504/1997: só pessoa
  física, limite de 10% dos rendimentos, recibo eleitoral, arrecadação nos meios
  oficiais — a plataforma não intermedeia pagamento). Fluxo: criar conta →
  vincular candidatura TSE → ativar. Revisada por URL direta (noindex, fora do
  menu) e então **publicada: entrou na nav do header e no rodapé, noindex
  removido** — o header segue em linha única com 8 itens até 1024px (verificado).
- **0.30.5-busca-formato-livre — a busca aceita o que você colar, como no Mastodon**:
  `GET /api/v1/federation/lookup` passa a aceitar, além de `@usuario@instancia`
  (o `@` inicial já era opcional), a **URL do perfil colada**
  (`https://instancia/@usuario`, `/users/usuario` ou o próprio actor URL) — fetch
  direto do Actor doc com content negotiation, sem WebFinger, igual ao colar-URL
  do Mastodon; documento sem `inbox` (post, coleção) é rejeitado com mensagem
  clara, e `http://` orienta a usar `https://`. O handle exibido vem de
  `preferredUsername@host` do actor id. Front (`/explorar` e Configurações →
  buscar no fediverso) valida os dois formatos e os textos de ajuda explicam.
- **0.30.4-busca-viva — sugestões carregam enquanto você digita**: a página
  `/buscar` ganhou typeahead — a partir de 2 caracteres, um debounce de 300 ms
  dispara o `GET /api/v1/search` existente e um dropdown mostra contas (avatar +
  handle), hashtags (com contagem de notas) e a ação "Buscar…" para os resultados
  completos. Navegável por teclado (↑/↓/Enter/Esc), respostas fora de ordem são
  descartadas por sequência, e o padrão ARIA de combobox anuncia a lista para
  leitores de tela. Nenhuma mudança de backend.
  Também em `/explorar` (a tela que motivou o pedido): a busca do fediverso, que
  só aceitava o endereço exato `@usuario@instancia`, agora sugere contas já
  conhecidas pela instância via `GET /api/v1/search/mentions` enquanto digita
  (funciona deslogado), com atalhos "Procurar no fediverso" (quando o texto é um
  endereço válido — o WebFinger continua exigindo login) e "Buscar em tudo"
  (leva pra `/buscar?q=…`).
- **0.30.3-threshold-ux — o formulário mostra a regra, não pede o número**: o campo
  "Limiar de apoios" saiu do formulário de propor; ao escolher o mandato, o form
  consulta o novo `GET /api/v1/threshold-preview?mandate_id=…` e exibe
  "🎯 Gatilho deste território: N apoios — 0,05% do eleitorado (M eleitores, fonte
  TSE)". Fecha a experiência do item 4: o autor entende a regra em vez de inventar
  um número que o servidor sobrescreveria.
- **0.30.2-embed — placar embedável para a imprensa** (item 7, fatia 1): novo
  `GET /embed/placar/{mandate_id}` na raiz — widget HTML autocontido (~2 KB, CSS
  inline, zero JS, cache 5 min) com respondidas × silêncios × taxa de resposta e
  link-fonte para o placar verificável; feito para iframe em portais e blogs (sem
  cabeçalhos anti-frame — circular é o ponto). O JSON para imprensa já existia na
  rota pública `GET /api/v1/scorecards/{mandate_id}`. Fatia 2 futura: OG card PNG
  por mandato/SLA (exige stack de rasterização).
- **0.30.1-threshold — gatilho proporcional ao eleitorado** (item 4 do plano): o
  autor não escolhe mais o threshold da própria proposta. Um middleware no
  composition root reescreve o campo no create com
  `clamp(⌈fração × eleitorado do território⌉, piso, teto)` — município para
  mandato municipal, UF para estadual/federal, nacional como fallback. Eleitorado
  oficial TSE na nova tabela `electorate` (migration 0522), populada por
  `scripts/seed-eleitorado-tse.py` do `perfil_eleitorado_ATUAL` (validado:
  157,8M nacional, SP 34,1M, capital paulista 9,1M — batem com o oficial; 5.571
  municípios). Config: `THRESHOLD_FRACTION` (default 0,05%), `THRESHOLD_FLOOR`
  (25), `THRESHOLD_CEIL` (10.000); território sem dado cai no piso (nunca
  bloqueia). Mesmo esforço relativo dispara consequência em Roraima e em São
  Paulo — legitimidade estatística do gatilho.
- **0.30.0-responder — reply-to-respond: o gabinete responde sem conta** (item 3 do
  plano): os e-mails de aviso ao gabinete passam a carregar um **link assinado**
  (`/responder/?sla=…&t=hmac_sha256(RESPOND_LINK_SECRET, sla_id)`) que abre a página
  de resposta pública SEM cadastro — a posse do token, entregue apenas à caixa
  oficial do mandato (dado público Câmara/Senado/TSE), é a autorização, como o AR
  postal. A página mostra a demanda e o prazo, aceita a resposta (com o opcional
  "compromisso concreto" → status `acted`) e registra via
  `ConsequenceService::respond` — desfecho permanente, SLA já resolvido responde
  409. Env ausente = recurso dormant (e-mails caem no link da proposta). Ataca o
  gargalo real do loop: a adoção pelo político, com atrito zero.
- **0.29.1-silencio-provado — a prova viaja com a denúncia** (item 2, fatia 2 —
  fecha o item): (1) a página da proposta ganha a seção **"Avisos ao gabinete —
  com recibo"**: cada tentativa datada, resultado e hash encadeado visíveis a
  qualquer visitante; (2) quando o SLA expira, o silêncio agora federa — a Note
  `#SilêncioRegistrado` publicada em nome do autor (mesmos gates e opt-out da Note
  de threshold, idempotente pela hashtag) carrega a linha do tempo dos avisos e o
  hash final da cadeia: a prova criptográfica viaja junto com a denúncia para
  Mastodon e todo o fediverso.
- **0.29.0-recibos — prova de notificação, o "AR digital do silêncio"** (item 2 do
  plano estratégico, fatia 1; migration 0521): todo e-mail ao gabinete vira um
  recibo persistido e **hash-encadeado por proposta** (`hash = sha256(prev|proposta|
  destinatário|tentativa|resultado|instante)`, genesis por proposta) — adulterar um
  recibo quebra a cadeia dali em diante e qualquer auditor reproduz os hashes na
  mão. Enquanto o SLA está `pending`, o worker **escala o aviso: D0 → D+1 → D+2**
  (máx. 3 tentativas; para quando o gabinete responde ou o prazo vence), cada
  reenvio com recibo próprio. Timeline pública em
  `GET /proposals/{id}/delivery-receipts`. O silêncio deixa de ser acusação e vira
  fato auditável. Fatia 2 (futura): embutir a cadeia na Note federada do silêncio
  e a timeline visual no ProposalDetail.
- **feat(eleicoes): pipeline TSE DivulgaCand pronto e ensaiado** (item 5 do plano
  estratégico) — `scripts/seed-candidaturas-tse.py` baixa o `consulta_cand_{ano}.zip`
  dos dados abertos, parseia os CSVs por UF (Latin-1) e gera SQL idempotente de
  upsert em `election`/`candidacy`; chave de upsert é o `SQ_CANDIDATO` do TSE
  (migration 0520, coluna `candidacy.tse_sq` + índice único parcial — o TSE
  republica os CSVs diariamente na janela de registro e o mesmo comando roda todo
  dia sem duplicar). **Ensaio com o dataset real de 2022**: 28.461 candidaturas
  ingeridas em 13s (13 presidenciáveis com nomes/números/status corretos, 33,9%
  mulheres — bate com o oficial), re-run idempotente (28.461 = 28.461), turnos 1/2
  separados por election. Em 15/08/2026 é um comando: `--year 2026` + psql.
- **test(coverage) lote 3: 51.2% → 54.2%, ratchet 50 → 53** — a superfície admin sob
  a mesma régua (anônimo 401, sessão comum 403, admin nunca 5xx) num loop sobre as 9
  listas admin (stats/users/peers/users-rich/reports/audit/webhooks/announcements/
  email-templates) + CRUDs: webhooks (criar/patch/deletar + evento inválido 400),
  ciclo de announcements (criar publicado → ativo pro cidadão → dismiss →
  despublicar), ações de moderação de conta (suspend/unsuspend/silence/unsilence),
  /me/admin-status refletindo o papel, e preview de convite enumeração-neutro
  (token desconhecido = 200 valid:false, nunca 500). 6 testes novos, 55 no total.
- **test(coverage) lote 2: 47.2% → 51.2%, ratchet 46 → 50** — mais 13 testes no
  harness (49 no total), agora sobre a superfície federada e auth: fluxo completo
  publicar nota (Mastodon-compat) → servir actor/outbox/followers/following em
  ActivityPub (com geração de chave do actor), instance/timelines públicos,
  registro de app OAuth + token com client inválido, CRUD de mutes/blocks/filters/
  lists, registro com CPF válido (202 verification_sent), rate-limit de login por
  IP (429 na 11ª), reset de senha resistente a enumeração, logout idempotente.
  Flakiness entre runs eliminada (IPs de teste aleatórios por execução — a
  auditoria de tentativas persiste no banco); suíte validada 2× seguidas.
- **test(coverage): 40.6% → 47.2%, ratchet 40 → 46** (issue #8, passo 2/4 do plano) —
  17 testes novos no harness oneshot `crates/gateway/tests/http_surface.rs` (36 no
  total), metade segurança / metade funcional: formulário de contato (setor fechado,
  honeypot, rate-limit 429 por IP), atestados (401/422/403 + roundtrip completo
  atestar→listar→revogar com operador de mandato real), gates de registro
  (CRUD admin gated, domínio bloqueado barra register de ponta a ponta, ip_rule
  deny nega login por CIDR e libera IP fora do range, CIDR inválido 400) e superfície
  fediverso anônima (webfinger, actor, verify_credentials, bookmarks). O ledger sqlx
  do banco de CI foi corrigido (0519 registrada via runner, não via psql).

### Fixed
- **mobile: fim do zoom-out — o site abria "miniaturizado" no celular**: dois
  elementos mais largos que a tela faziam o Chrome mobile renderizar a página
  inteira com zoom out (layout de 512 px numa tela de 390 px). Causas: (1) o form
  de `/buscar` não encolhia — o campo tinha piso intrínseco de ~378 px + botão de
  110 px; agora o combo tem `min-width: 0` + `flex-wrap` e o botão vira linha
  própria ≤480 px; (2) os blobs decorativos do HeroArt sangram de propósito
  (`inset` negativo) e vazavam 8 px — a section `.hero` ganhou `overflow: clip`.
  Guarda global: `overflow-x: clip` também no `html` — sem ele o do `body`
  propaga pro viewport e o body em si fica sem clip (pegadinha da spec).
- **ops(k8s): Secret fora do manifest aplicável** — postmortem 2026-07-10: um
  `kubectl apply -f deploy/k8s/gateway.yaml` sobrescreveu `DATABASE_URL`/`SMTP_*` de
  produção com os placeholders `CHANGE_ME` que viviam no mesmo arquivo (site fora do
  ar ~15 min; recuperação: rotação da senha do role `dsoc` no Postgres + restauração
  do SMTP a partir do env de dev; STORAGE_*/VAPID_* sobreviveram porque apply faz
  merge por chave). O Secret sai do `gateway.yaml` (agora seguro de aplicar) e vira
  `gateway-secrets.example.yaml`, bootstrap-only; updates só via `kubectl patch`.
- **0.28.5-reembed-fix** — dois achados do smoke da 0.28.4 em produção: (1) o boot
  frio carrega os dois modelos (~85 s) antes do listener e a **liveness probe matava
  o pod no meio do boot** (exit 137) — o manifest ganha `startupProbe` (até 5 min);
  (2) embedding de proposta **apagada** (purge de demo/LGPD art. 18) ficava em retry
  eterno no backlog do re-embed — agora NotFound purga a órfã (edge + embedding +
  centroide recomputado, ou cluster dissolvido se vazio). Manifest também passa a
  pinar a imagem corrente.

### Added
- **0.28.4-reembed** — fatia 2a do re-cluster: worker ganha o loop
  `reembed_backlog_loop` (`WORKER_REEMBED_MS`, default 60s; lotes de 8) que drena as
  rows de `consensus_embedding` com `text_sample` vazio — a era do stub FNV e o
  intervalo 0.27.x — regravando vetor com o modelo real, `direction_signature`
  (stance) e amostra NLI, e recomputando o centroide do cluster. Idempotente por
  construção (amostra preenchida sai do backlog); some sozinho quando o backlog
  seca. **Fronteira explícita**: membership NÃO é movida — reavaliar cluster de
  proposta re-embedada (com skip de clusters com SLA disparado) é a fatia 2b,
  porque mover emite eventos e mexe no gatilho de threshold.
- **0.28.3-atestado** — verificação de cidadania por web-of-trust (migration 0519),
  a camada custo-zero enquanto TSE/gov.br não respondem: operadores de mandato
  (`mandate_identity_binding`) e admins de partido aceitos podem **atestar
  publicamente** que conhecem um cidadão (`POST/DELETE /citizens/{id}/attestations`,
  lista pública em GET com flags do viewer). Atestado auditável (quem, quando, com
  que poder), revogável pelo próprio atestador, sem auto-atestado (CHECK no schema).
  Perfil público ganha o selo "🤝 Cidadania atestada por N" e o botão de
  atestar/revogar para quem tem o poder.
- **0.28.2-gates** — as regras de registro (migration 0514) passam a valer de fato; até
  aqui os CRUDs admin existiam mas nada as consultava no fluxo real. (1) Middleware no
  gateway (`signup_gates::gates_middleware`): register/register_politician recusam
  domínio em `email_domain_block` e IP negado por `ip_rule` (escopo signup/all); login
  recusa IP negado (escopo login/all); allow-pool vira allowlist; resposta 403 única e
  opaca (`gate_denied`), fail-open em erro de DB. (2) `GATEWAY_SIGNUP_REQUIRES_REVIEW=true`
  faz o confirm criar a conta com `pending_review=true` SEM emitir sessão (front mostra
  "falta a aprovação") e o login recusa com mensagem explícita até um admin aprovar em
  /admin/revisoes. CIDR matcher próprio (v4/v6, sem dependência nova) com testes; regra
  malformada nunca nega por acidente.
- **0.28.1-contato** — nenhum e-mail é mais publicado no site (os endereços anteriores em
  /sobre, /privacidade, /termos e /transparencia não existiam e eram alvo de harvesting):
  novo formulário único em `/contato` com setor pré-selecionado por link
  (`?setor=contato|lgpd|moderacao|seguranca`), enviado por `POST /api/v1/contact` via o
  relay SMTP soberano para a caixa interna (`CONTACT_INBOX`), com `Reply-To` de quem
  escreveu. Defesas do endpoint público: honeypot, rate-limit por IP
  (`CONTACT_RATE_MAX_PER_HOUR`, default 5/h) e validação de tamanho/setor.
  `SECURITY.md` passa a apontar para o formulário.
- **0.28.0-nli-judge** — o merge deixa de confiar em representações por-texto: crítica de
  usuário ("a linguagem é dinâmica; não dá para parametrizar palavras soltas — 'a grande
  obra do mestre Picasso' ≠ 'a pica de aço do mestre de obras'") verificada por medição:
  o par Picasso mede cosseno **0.068** (ABAIXO do threshold — mesclaria!) e os dois
  sentidos de "banco" 0.078, enquanto paráfrase legítima pode medir 0.116. Embedding
  comprime cada frase isoladamente; significado-em-contexto exige ler o PAR junto.
  Novo `nli_judge.rs`: cross-encoder NLI multilíngue (mDeBERTa-v3-xnli via candle, local,
  CPU) lê premissa+hipótese com cross-attention → implicação/neutro/contradição.
  Política calibrada em matriz pt-BR (limiar de confiança 0.5): contradição confiante
  veta (antagônicos medem 0.84–1.00); implicação confiante numa direção aceita (paráfrase
  creche: 0.98); resto (neutro = mesmo tópico, não mesmo pedido) NÃO mescla — mata
  homônimos, "mesma avenida outra obra" (cura a limitação documentada em 0.27.0) e
  pedidos de escopo diferente. Encanamento: trait `PairJudge` (domain), texto amostrado
  em `consensus_embedding.text_sample` (migration 0518), gate no ingest contra até 3
  membros do cluster, fail-open com log (juiz com erro = sem opinião; distância + stance
  continuam valendo). Env: `CONSENSUS_NLI_DIR` (~3-4s/par em release, pago só em
  candidatos raros a merge; RAM do pod 1.5Gi → 3Gi).
  Também: fixes de precisão no léxico de stance expostos pela mesma crítica —
  "contra" virou match exato (capturava "contraTAR" como negador!), "barrar" removido
  (colidia com "barragem"), exclusões para elevador/cortesia/cortina/vendedor/vendaval/
  fechadura/acabamento, com testes de regressão.
- **0.27.1-stance-guard** — veto de direção ideológica no clustering. Medição em pares
  cívicos pt-BR expôs o limite estrutural de QUALQUER sentence-embedder: "privatizar os
  postos do SUS" vs "proibir a privatização dos postos do SUS" medem cosseno 0.015 —
  abaixo de toda paráfrase legítima (0.072–0.089); "cortar orçamento da saúde" vs
  "aumentar orçamento da saúde" medem 0.046. Nenhum threshold separa posição política:
  embedding codifica TÓPICO, não direção — e mesclar antagônicos envenenaria o sinal (o
  SLA dispararia por uma demanda autocontraditória que o mandato poderia desqualificar).
  Novo `crates/platform/consensus/src/stance.rs`: léxico auditável e precision-first de
  eixos de política (scale±, public±, permit±, open±, staff±) + negadores que flipam o
  eixo seguinte ("proibir a privatização" ⇒ `public+`); cada proposta ganha uma
  `direction_signature` (migration 0517, coluna própria da crate) gravada no ingest; no
  candidato a merge, assinatura antagônica a qualquer membro do cluster VETA a mescla e
  forma cluster novo (log auditável `stance veto`). Defesa em camadas validada por teste:
  o par "vender o SUS" vs "valorizar radiologistas do SUS" é barrado pelo threshold
  (0.107 > 0.10); negação e direção orçamentária são barrados pelo veto. Falso-veto é o
  erro barato: cluster dividido mantém as duas demandas vivas; mescla errada corrompe
  ambas. Teste end-to-end no CI (stub): "aumentar X" vs "não aumentar X" formam clusters
  separados.
- **0.27.0-embeddings** — consenso semântico REAL (item nº 1 do plano estratégico): novo
  `ModelEmbedder` (`crates/platform/consensus/src/model_embedder.rs`, feature
  `model-embedder`) roda `intfloat/multilingual-e5-small` (384 dims — bate com o
  `vector(384)` da 0130) localmente via candle (Rust puro, CPU, zero rede na inferência —
  princípio 11 do PLAN). Prefixo `"query: "` simétrico, mean pooling sobre a attention
  mask, L2-norm. Seleção por env: `CONSENSUS_EMBEDDER=stub|model` (default stub),
  `CONSENSUS_MODEL_DIR` (default `/srv/model`), `CONSENSUS_QUERY_PREFIX` (`"query: "` para
  a família E5), `CONSENSUS_THRESHOLD` (default 0.15 stub / **0.10 model**, calibrado em
  pares pt-BR reais — paráfrases medem 0.079–0.089, pedidos distintos ≥0.134; limitações
  conhecidas documentadas no teste: sigla UBS≙posto fica de fora a 0.111, par
  mesmo-local-outra-obra falso-positiva a 0.078 → V2 rastreada em issue).
  Modelo carrega UMA vez por processo (OnceLock); falha de load ou de
  inferência degrada ruidosamente para o stub — o loop de consequência nunca trava.
  Deploy: artefatos em `/opt/dsoc/model` na VM (hostPath → `/srv/model`; a VM não alcança
  o HF por ser IPv6-only), limite de memória do pod 256Mi → 1536Mi. Antes: "creche no
  bairro" e "vaga em berçário" nunca clusterizavam (FNV hashing); agora clusterizam.
- **0.26.24-fase-E-autofed** — auto-federação server-side (Fase E completa do plan
  TSE-worthy): quando `ProposalThresholdCrossed` dispara, o `CivicNotifySub` publica uma
  Note pública (`Create(Note)` ActivityPub) **em nome do autor da proposta**, com título,
  link e `#DemocraciaBR` — antes só existia o banner com CTA manual. Gates: autor federável
  (`is_public` + `handle`), preferência `auto_federate_threshold` ligada (migration 0516,
  default true, opt-out na aba Preferências de `/configuracoes` + exposta em
  `GET/PATCH /me/preferences`) e idempotência via lookup no `federation_outbox_entry`
  (o dispatch é at-least-once). Keypair do actor gerado lazy se for o primeiro post.
  Best-effort: falha na federação nunca derruba a notificação in-app nem o dispatch loop.
- **0.26.0-fase-DE** — comparador de candidatos com dado (seed idempotente de 10 candidacies
  vinculadas aos 10 mandate.is_candidate=true, cobrindo as 4 elections estruturais);
  estatísticas públicas ao vivo na landing (`GET /stats/public` sem autenticação e sem PII —
  citizens_active, proposals_published, mandates_indexed, responses_public, silences_public,
  response_rate); banner âmbar destacado no `/propostas/:id` quando `thresholdCrossed`, com
  copy "o relógio começou a correr" + CTA "🌐 Amplifique no fediverso" (autor) ou
  "Compartilhar" (demais).
- **0.26.0-fase-FG** — 4 páginas institucionais com conteúdo real editorial:
  `/sobre` (tese, loop propor→responder/silenciar, PopSolutions, contatos DPO/moderação/security);
  `/privacidade` (LGPD completa: art. 7º base legal por finalidade + art. 18 direitos + cookies
  listados + retenção 30d); `/termos` (TOS + Contrato Social herdado do Decidim + rate limits
  publicados); `/transparencia` (código aberto em git.pop.coop, AGPLv3 + Cláusula Social,
  ADRs numerados, changelog público, dependências auditáveis — sem OpenAI/GCP em runtime).
  Endpoints LGPD (`crates/gateway/src/lgpd.rs`): `GET /me/lgpd/export` (JSON portável completo)
  + `POST /me/lgpd/delete-account` (soft-delete transacional em `citizen.deleted_at`, apaga PII,
  mantém conteúdo público anonimizado — LGPD art. 16). Migration 0154 `citizen.deleted_at`.
  Aba "LGPD" em `/configuracoes`; Footer com nav institucional; rodapé reforçado
  ("AGPLv3 · IPv6-first · Sem publicidade · Sem venda de dados"). Landing ganha barra de
  selos (🇧🇷 soberana · 🔓 AGPLv3 · 🚫 sem publicidade · 🌐 IPv6-first federado) + seção
  "Contadores ao vivo" com `LandingStats.svelte`.
- **0.26.0-fase-B-placar** — página pública dedicada `/politicos/[mandate]/placar` (SSG,
  ~1.663 novas páginas): card com % de resposta em fonte gigante colorida (verde/amarelo/vermelho),
  grid de 3 stats, empty state pra mandato sem SLA, botões Compartilhar (Web Share) + Publicar
  no fediverso. `MandateDetail` ganha CTA "📊 Placar público" ao lado de "Propor demanda".
  `MandatePanel` (Fase C polish) ganha 3 contadores no topo (com prazo correndo / respondidas /
  silêncio registrado) + link pro próprio placar público.
- **0.26.0-fase-A-eval** — fecha o loop `ProposalCreated → moderation.evaluate →
  ModerationCleared → publish_proposal`. Antes: propostas ficavam eternas em `status='draft'`
  porque ninguém disparava a moderação; nada aparecia publicamente. Fix: novo
  `ModerationEvaluateSub` no `worker.rs` consome `Event::ProposalCreated` e chama
  `moderation.evaluate(target=Proposal(id))`. `ModerationService::from_state(&AppState)`
  construtor de conveniência. `ProposalDto` ganha `status` + `published_at` (aditivos,
  retrocompat). Backfill em prod destravou 2 propostas antigas. Worker sobe com 14
  subscriptions (era 13).
- **0.25.0-fediverso-govbr** — skeleton do login gov.br (OIDC Authorization Code Flow),
  dormant enquanto não houver `GOVBR_CLIENT_ID`/`GOVBR_CLIENT_SECRET`. `GET /auth/govbr/start`
  gera state+nonce em cookie HttpOnly de 10min e redireciona pra `authorize` com escopos
  `openid profile email govbr_confiabilidades`. `GET /auth/govbr/callback` valida state (CSRF),
  troca code por tokens no `/token`, decoda id_token, valida nonce+aud, upsert citizen
  (novo com `verification_level='directory'`, ou update de `legal_name` se já existia via
  `govbr_sub`) + issue session cookie + 302 pra `/bem-vinda`. `GET /api/v1/auth/govbr/status`
  devolve `{enabled: bool}` pro front. `LoginForm.svelte` ganha botão azul gov.br oficial
  (#1351b4) acima do form quando enabled. Migration 0153: `citizen.legal_name` (nunca exposto
  na UI pública), `govbr_sub` (UNIQUE), `govbr_confiabilidade` (bronze|prata|ouro),
  `govbr_linked_at`. Débito conhecido: validação JWKS do id_token e mapeamento amr/acr →
  bronze/prata/ouro ficam pra fatia próxima (hoje decode-only, mitigado por TLS + state + nonce).
- **0.25.0 admin** — link "⚙️ Administração" no dropdown do perfil (`AuthMenu.svelte`),
  visível só pra owner/admin em `admin_role_binding`. `GET /me/admin-status` responde
  `{is_admin: bool}` (anônimo → `false`, sem vazar sinal). Cache em
  `localStorage.dsoc_is_admin` pinta imediato; revalida em background. GUI completa de
  usuários (`/admin/usuarios` — nova página dedicada) via `AdminUsersPage.svelte`: busca
  por nome/handle/email + 4 selects de filtro (partido / papel plataforma / papel partido /
  tipo cívico) + tabela rica (chips cidadão/político/candidato + título ✓ + privado) +
  drawer lateral de edição inline (um "Salvar" dispara 3 PATCHes em sequência: citizen →
  platform role → party role). Backend `crates/gateway/src/admin_users.rs`:
  `GET /admin/users-rich` (CTE com joins colapsa multi-role owner>admin>auditor),
  `PATCH /admin/users/{id}`, `PUT /.../platform-role`, `PUT /.../party-role`. Migration 0152
  `citizen.party_sigla` (filiação partidária opcional, só informativa).
- **0.25.0-templates** — templates de e-mail editáveis pela UI (Odoo-style). Migration 0151
  `email_template` (key PK, label, subject, body, default_subject/body, variables text[],
  updated_at/by), seed idempotente das 4 templates que a plataforma dispara. Novo
  `crates/gateway/src/email_templates.rs`: `render(db, key, vars) -> Option<(subject, body)>`
  com substituição `{{var_name}}` (parser mínimo, unknown key vira literal, 4 unit tests).
  Admin CRUD gated por `admin_role_binding IN ('owner','admin')`: `GET
  /admin/email-templates` + `PATCH /admin/email-templates/{key}` (payload
  `{subject?, body?, reset?}`) + `POST /admin/email-templates/{key}/preview`.
  `AdminConsole` ganha aba "E-mails" com `EmailTemplatesAdmin.svelte` (split view lista +
  form, chips clicáveis pra colar variáveis no cursor, botão "Voltar ao padrão", preview
  pré-populado). `proposal_delivery.rs` refatorado como demo pra usar render (fallback
  hardcoded se render → None).
- **0.25.0-delivery** — recibo de entrega da proposta: migration 0303 adiciona
  `proposal.notified_author_at` + `notified_mandate_at`. Novo `ProposalDeliverySub` no
  worker consome `ProposalCreated`, envia 2 e-mails via SMTP (autor: "sua proposta foi
  registrada" + mandato: "nova proposta cidadã"), grava timestamps em `IS NULL` guard.
  `ProposalDetail.svelte` mostra pro autor "✉️ E-mail entregue ao gabinete X em DD/MM HH:MM"
  + botão "Publicar no fediverso" que cria uma nota via `postNote()` com o link da proposta.
- **0.25.0-badge** — bell badge instantâneo em LeftRail + BottomNav via
  `window.dispatchEvent('dsoc-notifications-changed')` (disparado no clearAll do
  NotificationsFeed) e `navigator.serviceWorker.onmessage` (o `sw.js` faz `postMessage({type:
  'dsoc-push'})` após cada push recebido). Badges atualizam sem esperar poll de 60s.
  Guards `typeof window === 'undefined'` no `onDestroy` — Svelte 5 SSR chama cleanup no server.
- **0.25.0-push** — Web Push RFC 8291 end-to-end. Migration 0111
  `notify_web_push_subscription` (endpoint, p256dh, auth, user_agent, dead_at). Novo
  `crates/gateway/src/web_push.rs`: `POST /me/push-subscriptions` (persiste subscription do
  PushManager), `GET .../vapid-public-key` (503 sem VAPID), `pub async fn send_to_citizen(db,
  citizen_id, payload)` chamado por `civic_notify` após cada `user_notification` insert.
  410 Gone marca `dead_at`. Deps: `web-push = "0.10"` (hyper-client). Env
  `VAPID_PUBLIC_KEY` / `VAPID_PRIVATE_KEY` / `VAPID_SUBJECT` (gerados uma vez com
  `openssl ecparam -name prime256v1`). Frontend: `sw.js` mínimo, `webpush.ts` com
  enablePush/disablePush/isSubscribed, botão "Ativar push" no header do NotificationsFeed.
- **0.25.0-feed** — feed cidadão: migration 0411 expande `user_notification.kind` pra
  incluir 4 kinds cívicas (`proposal_threshold`, `sla_started`, `sla_response`, `sla_expired`).
  Novo `civic_notify.rs` com `CivicNotifySub` (2 subscriptions: Proposals + Consequence).
  Resolve `author_citizen_id` via join `SlaId → consequence_sla.proposal_id → proposal.author`,
  insere em `user_notification` com kind cívica + preview em pt-BR + `object_uri =
  /propostas/<id>`. `NotificationsFeed.svelte` ganha ícones/labels/tons pras 4 kinds cívicas
  (com fallback `?? 'info'/'bell'/kind` pra sobreviver a kinds novas antes do front deployar).
- **0.25.0-fediverso-urgente** — gate voto urgente por título eleitor. Migration 0302
  `proposal.urgencia` (`comum` | `urgente`). `VoteService::cast` lê `proposal.urgencia`; se
  urgente, lê `citizen.titulo_status`; se não bater `validated`/`verified`, retorna
  `Error::Forbidden` com mensagem específica pt-BR. `ProposalDto` ganha `urgencia`.
  ADR-0012 formaliza abandono do Zitadel/OIDC (log ERROR de startup vira INFO).
- **0.25.0-fediverso-defense** — migration 0107 `auth_login_attempt` (rate limit + auditoria).
  Rate-limit em `POST /auth/register` (3/h por IP via query em pending_signups) +
  `POST /auth/login` (10/h por IP via count em login_attempt). Nova rota
  `POST /auth/register/resend` reenvia link (enumeration-safe). Cleanup worker roda 1x/h
  (`WORKER_SIGNUP_CLEANUP_MS`) limpando pending_signup + login_attempt > 7 dias.
  `Error::RateLimit(String)` no dsoc-core (429 no auth). UX do título eleitor:
  aba `/configuracoes#identidade` com máscara 4-4-4 + status validado/verificado.
  `ProfileDto` ganha `titulo_status` — badge "🇧🇷 Título validado" no perfil público.
- **0.25.0-fediverso-verify** — verificação de e-mail obrigatória antes do cadastro virar
  conta. `POST /auth/register` (e `/register/politician`) passa a gravar um `auth_pending_signup`
  (migration 0106) com token SHA-256 e dispara `<origin>/confirmar-conta?token=…` via SMTP
  (mesmo relay do password-reset). Nova rota `POST /auth/register/confirm` redime o token e
  materializa citizen + credential + sessão numa única transação. CPF só é "consumido"
  depois da confirmação — bots com CPF válido não sujam mais a base. Nova página Astro
  `/confirmar-conta` (island `ConfirmSignupForm.svelte`) auto-submete o token e redireciona.
  Junto: `citizen.is_public` default virou `true` (padrão Mastodon, opt-out em Configurações).
- **0.25.0-fediverso-limits** — anti-spam da fatia federada: nota cap 5000 → 3000 chars, 1
  publicação a cada 15 min por cidadão (`POST /me/notes` retorna 429 c/ mensagem em pt-BR),
  voto de enquete rejeitado quando o `voter_url` não é local (`RemoteVoterForbidden` — enquete
  federa, apuração não). Regras aparecem no `/cadastrar` (RegisterForm), gate note no
  `PollView`. Playwright smoke cobre `/cadastrar` mostrando as 4 regras.
- **0.25.0 — Título de eleitor** (`crates/gateway/src/titulo_eleitor.rs`, migration 0105):
  `POST /me/titulo-eleitor` valida algoritmicamente (12 dígitos + 2 DVs TSE, com regra SP/MG)
  e grava `citizen.titulo_status='validated'`. `GET` devolve `{titulo_last4, titulo_status}`
  (LGPD-safe, sem número cheio). UNIQUE parcial em `titulo_eleitor` bloqueia sock-puppets.
- Foundational Cargo workspace: 23 crates across Tier 0–3 (PLAN.md §5.2), each with a `CRATE.md`
  contract describing responsibility, emitted/consumed events, and owned tables.
- Tier-0 contract crates `core`, `db`, `api-contract` (the freeze bottleneck).
- Baseline PostgreSQL schema + `pgvector` migration.
- CI/CD pipeline (Forgejo Actions): fmt, `clippy -D warnings`, `sqlx` checks against a real
  PostgreSQL service, per-crate tests + coverage, supply-chain audit (`cargo-deny`), Helm lint,
  and image build/release — established as the project's primary reliability & audit instrument.
- Kubernetes + Helm deployment chart (umbrella + per-service values), IPv6-first.
- Documentation set (English): architecture, parallelization, testing, CI/CD, deployment, ADRs, wiki.

- Wave 0 (ADR-0004): `EventBus` port + `RecordingEventBus`; 7 additive event variants + `Notify`
  topic + `NotificationId`; `dsoc-app` `AppState` wiring crate; migration registry + 3 CI guard
  scripts; per-crate `.sqlx` convention.
- ADR-0005 (Proposed): federate over **ActivityPub** (voter→candidate→official as one identity).

- Wave 1: 6 platform crates implemented + adversarially reviewed — events (PgEventBus + outbox
  dispatcher), auth (Zitadel OIDC + Authorization + verification levels, AP-readiness seam),
  notify (multi-channel fan-out), consensus (pgvector clustering), moderation (rules+stats),
  admin. Review caught + fixed a TOCTOU race, a notification-hijack authz hole, a phantom-UUID
  idempotency bug, and a dual-write hazard (now via ADR-0006 transactional outbox).

- Wave 2: the 6 consequence-loop / thesis crates — mandates (registry+onboarding), proposals
  (threshold trigger), votes (privacy-preserving tally), comments, consequence (SLA engine +
  public silence), scorecard (public projection). Each adversarially reviewed; review caught a
  recurring auth-bypass (citizen_id from body) and consumer non-idempotency, fixed at the
  contract level via ADR-0007 (dsoc_app::CallerId extractor + dsoc_db::consumed::claim_consumed).

- Wave 3: the 9 breadth crates — spaces (processes, assemblies, initiatives, consultations) and
  components (debates, meetings, budgets, surveys, accountability). Each adversarially reviewed;
  review caught a cross-tenant IDOR in surveys (publish/add_question now enforce org ownership)
  and corrected aspirational CRATE.md event contracts to match the frozen catalog.

- Front-end ↔ backend **contract tests** (web/tests/api.contract.test.ts, vitest) + CI
  (.forgejo/workflows/web.yml). Added after production bugs that such tests would have caught:
  (1) the register/login forms omitted `org_id` → Axum returned 422 text/plain that the client
  surfaced as 'falha de conexão'; (2) an absolute IPv6 API base caused cross-origin failures.
  Fixes: register/login centralized in api.ts (org_id can't be forgotten), relative same-origin
  base, defensive non-JSON response handling. Web served by the gateway behind Caddy/HTTPS at
  https://democracia.social.br; admin bootstrap documented (docs/ops/ADMIN.md).

### Decisions
- **ADR-0009**: web front-end = Astro + Svelte islands; SSG (static) now, SSR pod later.
- **ADR-0008**: sovereign CPF + e-mail/senha auth (Argon2id), reverses Zitadel for citizens.
- **ADR-0007**: authenticated-caller extractor + consumer idempotency ledger.
- **ADR-0006**: transactional outbox for atomic event emission.
- **ADR-0002**: reversed the original "no Docker / LXC + systemd" deployment stance to
  **Kubernetes + Helm**, justified per principle 12 (see `docs/decisions/`).

### Ported from Decidim (concepts, re-architected — not translated)
- Spaces/components separation → `crates/spaces` + `crates/components`.
- The Social Contract guarantee → `LICENSE-SOCIAL-CONTRACT.md`.

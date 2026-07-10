# Changelog

All notable changes to this project are documented here. Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
the project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Per PLAN.md principle 1, we **credit Decidim concepts we port**.

## [Unreleased]

### Added
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

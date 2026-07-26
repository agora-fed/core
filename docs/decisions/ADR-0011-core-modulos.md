# ADR-0011 — Core & Módulos: manifesto, verticais, permissões por chave e gate por org

- **Status:** Proposed
- **Contexto:** PLAN.md §8 (strangler-fig do gateway), ADR-0005 (federação/tiers), ADR-0006 (outbox), ADR-0007 (CallerId), ADR-0009 (SSG→SSR); varreduras completas de 2026-07: 57 módulos do gateway (30.8k LOC), 28 crates do workspace (~32k LOC), 101 migrations, 39 sítios de autorização, web SSG, estado do `admin_feature_flag`; pesquisas OCA (`__manifest__`, repos por tópico, ACL por módulo) e Decidim (`register_component`, permissions em cadeia).

## Contexto

Cinco fatos medidos motivam esta decisão:

1. **O gateway virou monólito acidental.** Dos 57 arquivos de `crates/gateway/src`, só 4 são composition-root legítimo (~1.4k LOC); **~29.4k LOC são domínio que vazou** — federação (11.6k), mandatos/accountability (3.3k), identidade (3.6k), admin (4.7k), eleições (1.7k), etc. `federation.rs` sozinho tem 4.147 LOC (limite: 800).
2. **Nada é desligável.** `api_router()` faz merge estático e incondicional de ~20 routers; `worker.rs` hardcoda 8 subscriptions + sweep de SLA incondicional. O `admin_feature_flag` existe de ponta a ponta (tabela, upsert, HTTP) mas tem **zero consumidores** — é dado inerte.
3. **Autorização real é só nível de verificação.** 32 sítios usam `authz.require(VerificationLevel)`; `admin_role_binding` (owner|admin|auditor) é gravado mas **nunca consultado**; `require_admin`, `forum_moderator` e `party_administrator` não existem como identificadores. **`platform/moderation` está 100% sem gate** (6 handlers abertos, incluindo `resolve_appeal`). Qualquer citizen Directory administra qualquer org.
4. **O vertical político está embutido no kernel.** O enum `Event` congelado em `dsoc-core` exige `MandateId` em `ProposalCreated` e `ClusterId` nos eventos de SLA — desligar mandates/consensus hoje quebra o catálogo central.
5. **Navegação e migrations não têm dono.** Header/Footer do web duplicam links hardcoded; migrations 0500–0546 são um grab-bag sem faixa de posse (federation estourou a faixa em 0410/0411); `check-fk-targets.sh` falha na main com 4 violações intra-crate de forums.

A boa notícia estrutural: o grafo de deps é raso e uniforme (todo crate de domínio depende só de {core, db, api-contract, app}), **não há FK cross-módulo fora do core** (só soft references + eventos), e 9 crates já são "quase-plugáveis" — o único impedimento é o merge estático de rotas.

## Decisão

### D1 — Definição do CORE

**Core = o que TODO deploy tem, sem flag, sem opção de desligar.** Critério: (a) outro módulo core depende dele em runtime, (b) obrigação legal/segurança de qualquer deploy exposto, ou (c) é o mecanismo de composição em si.

| Camada | Crates core | Justificativa |
|---|---|---|
| Kernel | `base/{core, db, api-contract, app}` + `gateway` | Composição. |
| Identidade & org | `platform/auth` | Middleware `inject_identity` depende dele; inclui signup gates, LGPD (obrigação legal art. 18), me/settings, convites. |
| Papéis & permissões | `platform/admin` | Orgs, `user_role`/`citizen_role_binding` (D4), feature flags (D5). É a fundação do framework plugável. |
| Eventos | `platform/events` | Espinha dorsal (outbox ADR-0006); indesligável enquanto houver qualquer efeito cross-crate. |
| Notificações & e-mail | `platform/notify` | Transporte SMTP + templates: todo e-mail da plataforma passa aqui (hoje 4 módulos importam SMTP de `proposal_delivery` — evidência de que é core no lugar errado). |
| Moderação & denúncias | `platform/moderation` | `proposals` só publica ao receber `ModerationCleared`; denúncias (note_report) são necessidade de qualquer deploy exposto. Core **mínimo**: regras + fila de reports + apelações. |
| Mídia | `platform/storage` | Porta genérica já existente. |

**NÃO é core** (módulo, gated por org via D5):

- **Federação (`verticals/federacao/*`)**: decisão explícita — federação é **módulo, não core**. É o crate mais desacoplado do workspace (depende só de api-contract) e nem está montado no gateway; uma prefeitura pode rodar Pindorama sem fediverso. Vira o módulo-vitrine da pluggability.
- **`consensus`**: módulo; exige fallback threshold-sem-cluster (trabalho contínuo, ver D6).
- **Todos os spaces e components**, incluindo `proposals` e `consequence`. A tese da plataforma (resposta-ou-silêncio) **não é core técnico — é o perfil "política-BR"**: um conjunto de módulos `default_enabled = true` no manifesto (proposals, votes, comments, consequence, scorecard, mandates). Distinção crucial: core = não desligável; perfil = ligado por padrão mas desligável por org.

### D2 — Manifesto de módulo em Rust

Cada crate de módulo exporta um manifesto estático + wiring de runtime, inspirado no `__manifest__.py` da OCA (metadado declarativo por módulo) e no `Decidim.register_component` (registry central + actions declaradas). Vive em `dsoc-app` (todo módulo já depende dele):

```rust
// crates/base/app/src/manifest.rs
pub struct ModuleManifest {
    pub id: &'static str,                       // "proposals"
    pub title: &'static str,                    // "Propostas" (pt-BR, vai pra UI admin)
    pub vertical: Vertical,                     // Participacao | Mandatos | Eleicoes | Federacao | Outreach
    pub kind: ModuleKind,                       // Space | Component | Client | PlatformOpt
    pub core: bool,                             // true = ignora flag, sempre on
    pub flag_key: &'static str,                 // "module.proposals" (convenção: module.<id>)
    pub default_enabled: bool,                  // perfil política-BR default
    pub depends_on: &'static [&'static str],    // ["mandates"] — ligar módulo exige deps ativas
    pub permissions: &'static [PermissionDef],  // chaves modulo.acao (D4)
    pub migration_ranges: &'static [(u32, u32)],// faixas de posse no REGISTRY (D6)
    pub nav: &'static [NavItem],                // itens de navegação pra UI
    pub page_prefixes: &'static [&'static str], // "/propostas" — SSG pula e gateway 404a se off
}

pub struct PermissionDef {
    pub key: &'static str,             // "proposals.create"
    pub min_level: VerificationLevel,  // pré-requisito ortogonal (Email/Directory/…)
    pub kind: PermKind,                // Participant (só nível) | Managed (exige papel, D4)
    pub label: &'static str,           // rótulo pt-BR da matriz de checkboxes
}

pub struct NavItem {
    pub label: &'static str,  // "Propostas"
    pub href: &'static str,   // "/propostas"
    pub slot: NavSlot,        // HeaderPrimary | Footer | AdminMenu
    pub order: i16,
}
```

O wiring de runtime fica separado (funções, não dados — routers Axum e handlers não são `'static`):

```rust
pub struct ModuleRuntime {
    pub routes: fn(AppState) -> axum::Router,
    pub admin_routes: Option<fn(AppState) -> axum::Router>,
    pub subscriptions: fn(AppState) -> Vec<Box<dyn dsoc_events::EventHandler>>,
    pub sweeps: fn(AppState) -> Vec<SweepTask>,   // ex.: SLA sweep do consequence
}
```

**Registro explícito no gateway** (`lib.rs`): `registry.register(dsoc_proposals::MANIFEST, dsoc_proposals::runtime())`. **Sem** `inventory`/`linkme` — auto-registro por linker é magia desnecessária num workspace fechado em compile-time (lição "o que NÃO copiar" do Decidim: a lista de módulos é fechada no build; o que é dinâmico é *quais estão ativos por org*, e isso é dado no Postgres). O gateway continua sendo o único lugar que conhece todos os módulos — mas agora conhece via **uma lista de manifestos**, não via 56 imports avulsos. Um teste de CI valida o registry: ids únicos, `depends_on` resolvível e acíclico, faixas de migração sem sobreposição, toda chave de permissão usada no código declarada em algum manifesto.

### D3 — Verticais estilo OCA, adaptadas a monorepo

A OCA agrupa módulos **por tópico funcional** em ~250 repositórios para que módulos relacionados sejam testados juntos, com PSC por repo. Adotamos o agrupamento por tópico, **rejeitamos multi-repo**: com time de 1 não há PSCs, não há necessidade de permissão de merge por diretório, e multi-repo custaria versionamento cruzado, CI duplicado e refactors não-atômicos — o monorepo dá refactor atômico, um grafo Cargo, um CI (que já compila a vertical inteira junta contra Postgres real, o mecanismo que a OCA usa para pegar conflito entre módulos irmãos). O que sobrevive da OCA num monorepo: **diretório por vertical = unidade de teste conjunto**, manifesto padronizado por módulo, permissões aditivas por módulo (D4), e faixa de migração com dono (D6).

Layout alvo (a distinção Decidim space/component vira metadado `kind` no manifesto, não diretório):

```
crates/
  base/            core, db, api-contract, app          (kernel)
  gateway/                                              (composition root)
  platform/        auth, admin, events, notify,
                   moderation, storage                  (CORE de serviço)
  verticals/
    participacao/  processes, assemblies, initiatives, consultations,
                   proposals, votes, comments, debates, forums,
                   meetings, budgets, surveys, consensus
    mandatos/      mandates, consequence, scorecard, accountability, opendata-br
    eleicoes/      elections, campaigns
    federacao/     federation, social-graph, mastodon-api
    outreach/      outreach
```

**Mapa: cada módulo do gateway → destino** (consolidado da varredura dos 57 arquivos):

| Fica no gateway (composition root) | `main.rs`, `lib.rs`, `worker.rs`, `rate_limit.rs` (este pode descer pra `base/app`) |
|---|---|
| **→ platform/notify** | `notifications.rs` (NotificationDto vira API pública), `civic_notify.rs`, `web_push.rs`, `email_templates.rs`, `mailer.rs`, `contact.rs`, **transporte SMTP extraído de `proposal_delivery.rs`** (`SmtpConfig`/`smtp_from_env`/`send_email` — destrava 4 migrações) |
| **→ platform/auth** | `govbr_oidc.rs`, `titulo_eleitor.rs`, `attestations.rs`, `invitations.rs`, `signup_gates.rs` (gates; ip_rules → moderation), `me_settings.rs`, `preferences.rs` (parte pessoal; server rules → admin), `whoami.rs` (agregador com deps injetadas), `lgpd.rs` |
| **→ platform/admin** | `admin_ext.rs`, `admin_users.rs`, `admin_content.rs`, `announcements.rs`, `public_stats.rs` |
| **→ platform/moderation** | `admin_reports.rs`, moderação de hashtag de `fediverso_admin.rs`, ip_rules de `signup_gates.rs` |
| **→ platform/events** | `webhooks.rs` |
| **→ platform/storage** | `note_media.rs` |
| **→ verticals/participacao/proposals** | `amendments.rs`, `threshold_policy.rs`, subscriber de `proposal_delivery.rs` |
| **→ verticals/participacao/forums** | `admin_forums.rs`, `forum_mailer.rs`, `forum_federation.rs` (dep opcional em federacao) |
| **→ verticals/participacao/consultations** | `consultas_ext.rs` (fundir com o crate existente, não viver paralelo) |
| **→ verticals/mandatos/mandates** | `politicos_ext.rs`, `politico_contacts.rs` |
| **→ verticals/mandatos/consequence** | `respond_link.rs`, `notification_receipts.rs` |
| **→ verticals/mandatos/scorecard** | `embed.rs`, `og_cards.rs` |
| **→ verticals/mandatos/accountability** | `reports.rs` (refactor do cache in-process anotado) |
| **→ verticals/mandatos/opendata-br** (novo) | `parlamentar_activity.rs` |
| **→ verticals/eleicoes/elections** (novo) | `elections.rs` |
| **→ verticals/eleicoes/campaigns** (novo) | `campanha.rs`, `campaign_groups.rs` |
| **→ verticals/federacao/federation** | `federation.rs` (fatiado em inbox/outbox/actor/delivery), `federation_feed.rs`, `discovery.rs`, `polls.rs`, emojis de `fediverso_admin.rs` |
| **→ verticals/federacao/social-graph** (novo) | `social_graph.rs` |
| **→ verticals/federacao/mastodon-api** (novo) | `mastodon_api.rs`, `mastodon_dto.rs`, `mastodon_oauth.rs` (migra por último no cluster) |
| **→ verticals/outreach/outreach** (novo) | `audience.rs`, `invite_campaign.rs`, `profile_nudge.rs` |

O movimento físico dos crates existentes para `verticals/` é um `git mv` + ajuste de paths do workspace, **um commit por vertical**, feito de forma contínua (não bloqueia R0).

### D4 — Permissões como registro de chaves `modulo.acao`

Rejeitamos bitmask fixa (estilo Mastodon puro) porque o conjunto de permissões **não é fechado**: cada módulo declara as suas no manifesto e a matriz cresce quando um módulo é instalado — o modelo é a ACL aditiva por módulo da OCA (`ir.model.access` por grupo) com a **hierarquia por posição** do Mastodon.

**Schema** (migration nova na faixa do admin):

```sql
CREATE TABLE user_role (
  id          uuid PRIMARY KEY,
  org_id      uuid NOT NULL REFERENCES org (id),
  name        text NOT NULL,
  position    integer NOT NULL DEFAULT 0,          -- hierarquia estilo Mastodon
  permissions text[] NOT NULL DEFAULT '{}',        -- chaves modulo.acao
  is_system   boolean NOT NULL DEFAULT false,      -- seeds: Proprietário/Admin/Moderador/Auditor
  UNIQUE (org_id, name)
);
CREATE TABLE citizen_role_binding (
  org_id     uuid NOT NULL REFERENCES org (id),
  citizen_id uuid NOT NULL REFERENCES citizen (id),
  role_id    uuid NOT NULL REFERENCES user_role (id) ON DELETE CASCADE,
  granted_by uuid REFERENCES citizen (id),
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (org_id, citizen_id, role_id)
);
```

**Regras:**

1. **Duas classes de ação** (do inventário de 39 sítios): `Participant` (votes.cast, comments.create, initiatives.sign, surveys.respond, budgets.participate…) continua gated **só por VerificationLevel** — cidadão não precisa de papel para participar. `Managed` (surveys.manage, meetings.minutes, assemblies.members.manage, moderation.*, roles.manage, flags.manage, mandates.offices.manage, consequence.sweep…) exige **nível E chave no papel**. O nível de verificação é pré-requisito **ortogonal** — nunca substituído pelo papel.
2. **Deny por padrão para chaves `Managed`**: handler sem chave declarada no manifesto = teste de CI falha (o "modelo sem ACL" da OCA); chave não concedida a nenhum papel do ator = 403.
3. **Hierarquia por posição (Mastodon)**: um ator só cria/edita/atribui papéis com `position` estritamente menor que a sua maior posição, e só gerencia usuários abaixo de si. Papel-sistema "Proprietário" (posição máxima, seed no create_org) tem implicitamente todas as chaves.
4. **Matriz de checkboxes gerada, não hardcoded**: a UI admin monta linhas = papéis da org, colunas = `PermissionDef` dos **módulos ativos** da org (registry de manifestos × flags efetivas, D5), com `label` pt-BR do manifesto. Instalar módulo adiciona colunas; desligar módulo esconde as colunas (as chaves ficam inertes no `text[]`, sem efeito — tolerância a chave desconhecida).
5. **Enforcement**: extractor/guard `RequirePermission("modulo.acao")` em `dsoc-app`, sobre o `CallerId` (ADR-0007 — nunca identidade do body; corrige de passagem votes/comments que hoje leem do body). Cadeia curta deny-wins: flag do módulo (D5) → nível → chave.
6. **Migração dos papéis atuais**: `admin_role_binding` (owner|admin|auditor) vira seed determinístico de `user_role` + bindings; tabela antiga é congelada e removida em release posterior.
7. **Prioridade de enforcement** (gaps críticos do inventário): `moderation.*` (hoje 6 handlers sem gate nenhum) e `roles.manage`/`flags.manage`/`orgs.manage` (hoje qualquer Directory-level muta qualquer org) entram **antes** de qualquer flag virar load-bearing.

### D5 — Gate por org via `admin_feature_flag` efetivo

O mecanismo já existe (migration 0150, upsert idempotente, HTTP) — falta o consumo. Decisões:

1. **Convenção de chave**: `module.<id>` (bate com `validate_flag_key`). Registro canônico = os `flag_key` dos manifestos; flag fora do registro é ignorada pelo gate.
2. **Flag efetiva** = linha em `admin_feature_flag` se existir; senão **`default_enabled` do manifesto** (default-open para o perfil política-BR — não quebra orgs existentes; o DEFAULT `false` do schema fica irrelevante porque ausência de linha cai no manifesto). `core: true` ignora flag — sempre on, a UI admin nem mostra o toggle. Ligar módulo com `depends_on` inativo é rejeitado com 422.
3. **Gate de rotas no gateway**: `api_router()` passa a iterar o registry e envolver cada router de módulo com `flag_gate(manifest, state)`. Resposta para módulo off: **404** (não revela existência). Resolução de org **no mesmo lugar que o handler usa**: `CallerId`/header para mutações, `org_id` de query/path para GETs públicos — nunca gatear pelo header enquanto o handler opera no org do body (bypass mapeado na varredura; fechar junto a divergência sessão-org × body-org).
4. **Gate no worker**: eventos carregam `org_id`; o dispatch consulta a flag efetiva do módulo dono do subscriber e **pula** handlers de módulos desligados naquela org (cursor avança — skip, não retry). Sweeps (`SweepTask`, ex. SLA) filtram por orgs com o módulo ativo, substituindo o spawn incondicional atual.
5. **Cache**: flags efetivas em cache in-process com TTL curto (30–60s), invalidado pelo novo evento `admin.flag_changed` emitido no `set_feature_flag` (o bus já está injetado no admin e hoje não emite nada).
6. **Nav e páginas**: novo endpoint público leve `GET /api/v1/orgs/{org}/modules` → módulos ativos + `NavItem`s + `page_prefixes`, montado do registry × flags. O web consome isso como **fonte única** de navegação (Header e Footer param de duplicar listas hardcoded). Enquanto o site for SSG (ADR-0009), esconder link não basta: o gateway 404a as rotas de API do módulo off, e a migração das páginas de módulo para SSR/híbrido lendo a config da org fica como pré-requisito do multi-org real no front (já decidido em ADR-0009).
7. **Leituras de flags** deixam de ser públicas: viram `admin.flags.view` (papel auditor+) — quando a flag gateia módulo de verdade, enumerar configuração é info-leak.
8. **Federação**: quando `verticals/federacao` for montado, o gate se aplica **também na serialização de objetos AP** (outbox/objects), não só nas rotas — conteúdo de módulo desligado não pode vazar via fediverso; o que já federou não é retratável (limitação documentada).
9. **Gate de rota ≠ gate de dado**: desligar módulo não remove linhas nem projeções já feitas (scorecard, notificações, events_log). Aceito e documentado; purga é operação administrativa separada, fora de escopo.

### D6 — Migração incremental, sem big-bang

**Princípio**: nenhum release quebra o deploy k3s atual. O manifesto entra **por cima** dos crates existentes; extrações do gateway acontecem módulo a módulo na ordem ditada pelos dois hubs de acoplamento (SMTP primeiro; cluster federação por último e como unidade).

**R0 entrega (a fundação, ~1 release):**
- `ModuleManifest`/`ModuleRuntime` + registry no gateway com manifesto para **todos os crates existentes** (rotas e subscriptions passam a vir do registry — comportamento idêntico).
- Extração do transporte SMTP de `proposal_delivery` → `platform/notify` (destrava contact, forum_mailer, profile_nudge, mailer).
- Tabelas `user_role`/`citizen_role_binding` + `RequirePermission` + enforcement nos gaps críticos (moderation.*, roles/flags/orgs.manage) + seed dos papéis a partir de `admin_role_binding`.
- `flag_gate` no router + gate no dispatch do worker + cache + `admin.flag_changed`, cobrindo de imediato os 9 crates quase-plugáveis (processes, assemblies, initiatives, consultations, debates, meetings, budgets, surveys, accountability).
- `GET /orgs/{org}/modules` + fonte única de nav no web.
- Governança de migrations: fk-allow.txt consertado (3 linhas), REGISTRY.md atualizado com mapa arquivo→módulo do grab-bag 05xx, faixas novas de largura 50 a partir de 0600 com posse declarada em `migration_ranges`; permanece a **opção A** (migrator único + gate de runtime) enquanto o tenancy for DB único.

**Contínuo (pós-R0, sem prazo acoplado a release):**
- Extrações do gateway conforme o mapa de D3 — ordem: (1) migrações fáceis sem deps (amendments, social_graph, politicos_ext, embed/og_cards, elections…); (2) auth/admin/notify; (3) fatiamento de `federation.rs` e migração do cluster federação como unidade, `mastodon-api` por último.
- Genericização do catálogo de eventos (`MandateId` → `RecipientId`, `ClusterId` opcional em SLA) — pré-requisito para desligar mandates/consensus de verdade; fallback threshold-sem-cluster no consequence.
- Quebra do `api-contract` monolítico em contratos por módulo; movimento físico para `verticals/`; SSR das páginas de módulo; arquivos >800 LOC fatiados ao migrar.

**Issues R0** (título + corpo curto):

1. **notify: extrair transporte SMTP de proposal_delivery** — Mover `SmtpConfig`/`smtp_from_env`/`send_email` para `platform/notify`; atualizar contact, forum_mailer, profile_nudge, mailer, proposal_delivery para importar de notify. Nenhuma mudança de comportamento; destrava 4 migrações futuras.
2. **app: struct ModuleManifest + ModuleRuntime + registry** — Tipos em `base/app` (D2); teste de CI valida ids únicos, deps acíclicas, faixas de migração sem overlap.
3. **gateway: montar rotas e subscriptions via registry** — `lib.rs` e `worker.rs` iteram o registry em vez de imports avulsos; manifesto mínimo para os 21 crates de domínio. Zero mudança de comportamento (snapshot de rotas OpenAPI idêntico como teste).
4. **admin: tabelas user_role + citizen_role_binding + seed** — Migration na faixa do admin; seed determinístico de owner|admin|auditor a partir de `admin_role_binding`; papel-sistema Proprietário por org.
5. **app: guard RequirePermission + registro de chaves** — Extractor sobre CallerId; cadeia flag→nível→chave; hierarquia por posição na gestão de papéis.
6. **moderation: fechar os 6 handlers sem gate** — CallerId obrigatório; `moderation.rules.manage`, `moderation.decisions.view`, `moderation.appeals.resolve` como Managed; `moderation.appeals.file` exige cidadão autenticado dono do conteúdo. **CRÍTICO — primeiro da fila.**
7. **admin: enforcement de papel em orgs/roles/flags.manage** — `authorize_mutation` passa a exigir chave via papel, não só Directory; fecha o "qualquer Directory administra qualquer org" antes de flags virarem load-bearing.
8. **gateway: middleware flag_gate por módulo** — Flag efetiva (linha ou `default_enabled`), 404 para off, resolução de org idêntica à do handler, cache TTL + invalidação por `admin.flag_changed`, cobertura imediata dos 9 crates quase-plugáveis.
9. **worker: gate de subscriptions e sweeps por org** — Dispatch pula handler de módulo off no org do evento (cursor avança); SLA sweep filtra orgs com consequence ativo.
10. **api: GET /orgs/{org}/modules** — Endpoint público leve com módulos ativos, NavItems e page_prefixes, montado do registry × flags.
11. **web: fonte única de navegação** — Header e Footer consomem /modules (island); remove as duas listas hardcoded; seções da landing viram blocos condicionais por módulo ativo.
12. **votes/comments: identidade do CallerId, não do body** — Corrigir a violação de ADR-0007 nos dois components ao ligar o RequirePermission; org da sessão prevalece sobre org do body.
13. **migrations: governança** — Adicionar 3 linhas ao fk-allow.txt (0541/0542/0545); REGISTRY.md com mapa arquivo→módulo de 04xx/05xx; reservar faixas de largura 50 a partir de 0600 conforme `migration_ranges`.
14. **e2e: contrato do perfil política-BR** — `core_loop.rs` vira teste do perfil: roda com módulos default-on e valida que desligar um módulo não-core (ex. debates) 404a rotas e pula subscribers sem quebrar o loop.

## Consequências

**Positivas**
- O gateway volta a ser composition root: conhece módulos por **uma lista de manifestos**, e as ~29.4k LOC vazadas têm destino declarado e ordem de extração definida.
- "Instalar módulo por org" vira real em três camadas coerentes: rotas (404), worker (skip) e UI (nav + matriz de permissões), tudo derivado do mesmo manifesto — mudou o manifesto, mudou o produto.
- Autorização ganha enforcement onde hoje não existe (moderação, gestão de papéis/flags) e um modelo aditivo que cresce com módulos sem editar os existentes (lição OCA), preservando VerificationLevel como pré-requisito ortogonal (ADR-0005).
- Time de 1 mantém refactor atômico, um CI, um deploy; nada de multi-repo.

**Negativas / aceitas**
- **Gate de rota ≠ gate de dado**: tabelas de módulos off existem vazias (opção A de migrations) e dados já projetados (scorecard, notificações, fediverso) não somem ao desligar. Documentado; purga é operação à parte.
- O vertical político continua no kernel (`MandateId` no enum Event) até a genericização contínua — desligar `mandates` de verdade ainda não é possível no R0; o que o R0 dá é o *mecanismo* de desligamento para os 9 módulos limpos.
- `api-contract` segue monolítico no R0 (OpenAPI expõe shapes de módulos off) — assumido, quebrar por módulo é trabalho contínuo.
- Duplo lookup por request (flag + papel) mitigado por cache TTL; custo residual aceito.
- Enquanto o web for SSG, páginas de módulo off existem fisicamente no build da org default; a proteção real está na API (404). Multi-org no front depende do SSR já decidido no ADR-0009.
- Migração de papéis exige cuidado: janelas entre seed de `user_role` e remoção de `admin_role_binding` mantêm as duas tabelas; a antiga fica read-only até o release de remoção.

**Follow-ups**: issues R0 acima; depois, na ordem dos hubs: extrações fáceis → auth/admin/notify → fatiar e migrar federação → `RecipientId` no catálogo de eventos → api-contract por módulo → mover crates para `verticals/`.
---

## Emendas incorporadas — revisão adversarial (2026-07-26)

A revisão adversarial (workflow ultraplan, 10 agentes) validou a direção e derrubou pontos
concretos. As correções abaixo são **normativas** e prevalecem sobre o texto acima onde conflitam:

1. **`mandate` é dado core.** A tabela está no baseline (0001) com FK NOT NULL de 7+ módulos —
   o *registro* de mandatos é core como org/citizen; só o *workflow* (onboarding, convites,
   verificação) é módulo. A genericização `MandateId → RecipientId` é migração de schema, cara,
   e fica fora do R0.
2. **Gate por org no extractor, não em middleware de router.** Os "9 quase-plugáveis" recebem
   `org_id` no body — middleware não vê body sem custo/bypass. O gate de módulo vive no mesmo
   extractor `RequirePermission` que resolve o org que o handler vai usar.
3. **Cache só de flags (TTL 30s); grants de papel SEMPRE no banco.** Cachear grants criaria
   janela de revogação em moderação — inaceitável. Ações `Participant` (votes.cast etc.)
   curto-circuitam sem tocar `citizen_role_binding`.
4. **Manifesto enxuto** — cortados `vertical`, `kind` e `migration_ranges` (sem consumidor no
   R0; posse de migração fica no REGISTRY.md). **Sem `git mv` para `verticals/`** — a taxonomia
   platform/spaces/components permanece; vertical é metadado futuro se houver consumidor.
5. **Fila de segurança destravada de qualquer fundação nova.** Os gates ausentes fecham com o
   mecanismo atual (`admin_role_binding` + headers) ANTES do manifesto/user_role — o primeiro
   (superfície /moderation inteira sem auth) foi corrigido no ato: hotfix `2197be9`, imagem
   `0.59.2-moderation-gate` em produção com verificação anônimo→401.
6. **Complementos:** reservar faixa 0600–0649 pro core no REGISTRY; check de CI "migração nova
   > maior número na main"; saneamento de linhas `module.*` pré-existentes em
   `admin_feature_flag` antes do gate ligar; SSG falha alto se módulo declarado ativo responder
   404; skip de evento com módulo desligado é perda permanente (documentado, replay fora de
   escopo); snapshot tests da superfície AP são pré-requisito pra fatiar `federation.rs`;
   `GET /orgs/{org}/modules` é público por design (D5.7 protege só as linhas cruas de flags).

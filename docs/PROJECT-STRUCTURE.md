# Project structure

Annotated repository tree. The **tier system** (PLAN.md §5.2) is the law of the workspace:
a crate may depend only on lower tiers, and cross-crate *effects* travel exclusively through
the durable event log (`events_log`) or the gateway. `scripts/check-crate-boundaries.sh`
enforces this in CI.

```
democracia-social/
├── PLAN.md                     # engineering north star — frozen principles, read first
├── README.md                   # entry point (you are here → parent)
├── CHANGELOG.md                # keep-a-changelog, cited publicly on /transparencia
├── CONTRIBUTING.md             # workflow, language policy (code EN / civic content PT)
├── SECURITY.md                 # disclosure policy
├── LICENSE                     # AGPL-3.0-or-later
├── LICENSE-SOCIAL-CONTRACT.md  # Decidim-inherited social contract clause
├── Cargo.toml                  # workspace root: members, [workspace.lints], deps
├── rust-toolchain.toml         # pinned toolchain (transitive deps need >= 1.86)
├── clippy.toml / rustfmt.toml / deny.toml / .editorconfig
│
├── .forgejo/workflows/         # CI on the self-hosted Forgejo runner
│   ├── ci.yml                  #   fmt · clippy -D warnings · boundary guards ·
│   │                           #   workspace tests (real PostgreSQL) · coverage gate
│   ├── security.yml            #   cargo-deny · cargo-audit · secret scan (+ weekly cron)
│   ├── web.yml                 #   front-end contract tests + SSG build
│   ├── helm.yml                #   chart lint/template/kubeconform
│   └── release.yml             #   tagged releases
│
├── .config/
│   ├── settings.env.example    # template for local secrets
│   └── settings.env            # real secrets — gitignored, NEVER committed
│
├── crates/                     # ───────── the Rust workspace (tiered) ─────────
│   │
│   │  ── Tier 0 · frozen contracts (single owner, changes need ADR) ──
│   ├── core/                   # dsoc-core: ids (CitizenId, OrgId…), Error, Event enum,
│   │                           #   topics — the shared vocabulary of every crate
│   ├── db/                     # dsoc-db: pool bootstrap, Db alias, migration policy
│   ├── api-contract/           # dsoc-api-contract: DTOs + ApiResponse envelope + OpenAPI
│   │
│   │  ── shared application state ──
│   ├── app/                    # dsoc-app: AppState (db, clock, object storage ports),
│   │                           #   CallerId extraction — what every service is built from
│   │
│   │  ── Tier 1 · platform crates ──
│   ├── platform/
│   │   ├── auth/               # sovereign signup/login (Argon2id + CPF), sessions,
│   │   │                       #   citizen profiles, actor keypairs, LGPD helpers,
│   │   │                       #   password reset, signup e-mail verification
│   │   ├── notify/             # channels (e-mail SMTP, future WhatsApp), SLA notices
│   │   ├── events/             # the bus: events_log + per-subscriber durable cursors
│   │   ├── consensus/          # embeddings (pgvector) + clustering — the signal merger
│   │   ├── moderation/         # rules, evaluation, audit-logged decisions
│   │   ├── admin/              # admin_role_binding, org administration
│   │   └── storage/            # object storage port (MinIO/S3) for avatars & media
│   │
│   ├── gateway/                # dsoc-gateway: THE public HTTP surface (Axum) + the
│   │   └── src/                #   background worker (14 event subscriptions, SLA sweep,
│   │                           #   federation delivery). Composes every crate's routes
│   │                           #   under /api/v1 and serves web/dist at the same origin.
│   │                           #   Gateway-owned modules: mastodon_api/oauth, federation
│   │                           #   S2S surface, admin console APIs, elections, LGPD,
│   │                           #   gov.br OIDC, web push, dashboards, civic notifications
│   │
│   │  ── Tier 2 · participation spaces ──
│   ├── spaces/
│   │   ├── mandates/           # the 70k real political mandates + parties catalogue —
│   │   │                       #   the anchor of accountability
│   │   ├── processes/          # participatory processes (skeleton)
│   │   ├── assemblies/         # assemblies (skeleton)
│   │   ├── initiatives/        # citizen initiatives (skeleton)
│   │   └── consultations/      # consultations (skeleton)
│   │
│   │  ── Tier 2 · participation components ──
│   ├── components/
│   │   ├── proposals/          # citizen demands: create → moderate → publish → threshold
│   │   ├── votes/              # support votes (aggregate only, never per-citizen linkage)
│   │   ├── comments/           # threaded comments under proposals
│   │   ├── consequence/        # ★ SLA engine: clock, respond/expire, write-once outcome
│   │   ├── scorecard/          # ★ permanent public record per mandate
│   │   ├── accountability/     # official activity data surfaces
│   │   ├── debates/            # deliberation (skeleton — future Pol.is-style opinion map)
│   │   └── meetings/ budgets/ surveys/   # remaining Decidim-parity components (skeleton)
│   │
│   │  ── Tier 3 · clients ──
│   └── clients/
│       └── federation/         # dsoc-federation: ActivityPub vocabulary, actors,
│                               #   HTTP signatures, webfinger/nodeinfo, AP mapping
│
├── web/                        # ───────── front-end (Astro SSG + Svelte islands) ─────────
│   ├── src/
│   │   ├── pages/              # Astro routes → 3,400+ static pages (politicians, placar,
│   │   │                       #   propostas, eleições 2026, feed, admin, institucional)
│   │   ├── components/
│   │   │   ├── islands/        # interactive Svelte islands (browsers, panels, admin)
│   │   │   └── ui/             # design-system primitives (Card, Icon, tokens)
│   │   ├── layouts/            # shared shells (nav, footer, meta)
│   │   ├── lib/                # api.ts (typed client), parties.ts, toasts, stores
│   │   └── styles/             # design tokens, dark mode
│   ├── tests/                  # vitest contract tests + Playwright UI smoke
│   └── public/                 # manifest, sw.js (push), brand assets
│
├── migrations/                 # append-only numbered SQL (registry: REGISTRY.md)
│                               #   0001 baseline · 01xx auth/notify/events… · 02xx spaces
│                               #   03xx components · 04xx federation · 05xx product waves
│                               #   Applied MANUALLY in prod (owner→dsoc rule; see
│                               #   docs/ops/DEPLOYMENT-k3s-vm.md)
│
├── deploy/
│   ├── docker/Dockerfile       # production gateway image (binary + web/dist baked in)
│   ├── k8s/gateway.yaml        # the live single-node k3s manifests (hostNetwork)
│   └── helm/                   # chart for the multi-replica future (ADR-0002)
│
├── docs/
│   ├── ARCHITECTURE.md         # runtime architecture & crate graph
│   ├── PROJECT-STRUCTURE.md    # this file
│   ├── PLANO-ESTRATEGICO-2026.md  # product strategy, election-cycle sequencing (pt-BR)
│   ├── ROADMAP.md              # delivery roadmap (4 fases)
│   ├── TESTING.md              # test strategy: unit + integration on real PostgreSQL
│   ├── CICD.md                 # the pipeline as an audit instrument
│   ├── PARALLELIZATION.md      # how many agents work in parallel without colliding
│   ├── decisions/              # ADR-0001… — every principle reversal is justified
│   ├── ops/                    # deployment runbooks (k3s VM, Helm, admin bootstrap)
│   └── wiki/                   # user/steward-facing wiki (mirrored to Forgejo wiki)
│
├── scripts/                    # operational + CI-guard scripts
│   ├── check-crate-boundaries.sh   # no cross-crate imports outside the tier rules
│   ├── check-migration-numbers.sh  # unique, monotonic migration numbers
│   ├── check-fk-targets.sh         # cross-crate FKs only to identity tables (+ fk-allow.txt)
│   ├── check-lints-optin.sh        # every member opts into [workspace.lints]
│   ├── scan-secrets.sh             # no committed credentials
│   ├── seed-parlamentares-reais.py # Câmara/Senado open-data seed (594 federal)
│   ├── seed-deputados-estaduais.py # ALESP/ALMG/TSE 2022 seed (1,059 state)
│   └── bootstrap-admin.sh          # first-admin creation runbook helper
│
└── tests/                      # cross-crate integration harness (workspace-level)
```

## Conventions that keep this healthy

- **One crate = one owner = one bounded context.** If two crates need each other's tables,
  the design is wrong — emit an event instead.
- **Migrations are immutable once applied** (sqlx checksums); fixes are new migrations.
  Every new table ends with `ALTER TABLE … OWNER TO dsoc;` (prod applies as `postgres`).
- **Gateway is the only public surface.** Domain crates expose `routes(state)`; the gateway
  merges them under `/api/v1` and owns the auth middleware that injects
  `x-dsoc-citizen-id`/`x-dsoc-org-id`.
- **Front-end talks only to `/api/v1` on the same origin** — no CORS anywhere.
- **English in the repo, Portuguese to the citizen.** UI copy, civic docs and the strategy
  plan are Portuguese; code, comments, commits and engineering docs are English.

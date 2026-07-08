# Changelog

All notable changes to this project are documented here. Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
the project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Per PLAN.md principle 1, we **credit Decidim concepts we port**.

## [Unreleased]

### Added
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

# Changelog

All notable changes to this project are documented here. Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
the project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Per PLAN.md principle 1, we **credit Decidim concepts we port**.

## [Unreleased]

### Added
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

### Decisions
- **ADR-0007**: authenticated-caller extractor + consumer idempotency ledger.
- **ADR-0006**: transactional outbox for atomic event emission.
- **ADR-0002**: reversed the original "no Docker / LXC + systemd" deployment stance to
  **Kubernetes + Helm**, justified per principle 12 (see `docs/decisions/`).

### Ported from Decidim (concepts, re-architected — not translated)
- Spaces/components separation → `crates/spaces` + `crates/components`.
- The Social Contract guarantee → `LICENSE-SOCIAL-CONTRACT.md`.

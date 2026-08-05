# AGORA

> **Democratic infrastructure framework** — deliberation, decision, and
> **consequence**, natively federated on the Fediverse.
> Stewardship: **PopSolutions Software & Comunicação LTDA, a cooperatively
> managed company** · License: **AGPL-3.0-or-later**
> (+ [Social Contract](./LICENSE-SOCIAL-CONTRACT.md))

[![CI](https://github.com/agora-fed/core/actions/workflows/ci.yml/badge.svg)](https://github.com/agora-fed/core/actions/workflows/ci.yml)
[![Security](https://github.com/agora-fed/core/actions/workflows/security.yml/badge.svg)](https://github.com/agora-fed/core/actions/workflows/security.yml)
[![codecov](https://codecov.io/gh/agora-fed/core/branch/main/graph/badge.svg)](https://codecov.io/gh/agora-fed/core)

**AGORA** is the framework: a country-agnostic, Rust-based engine for
participatory democracy. **Installations** localize it. The reference
installation is **PINDORAMA** (<https://democracia.social.br>), running AGORA
with the Brazilian localization module
[`agora-fed/l10n-brazil`](https://github.com/agora-fed/l10n-brazil) —
~70,000 real mandates (federal, state, municipal) indexed from official open
data, each with a public accountability scorecard.

**Thesis:** *Participation without consequence is theater.* AGORA converts
citizen demand into **visible, time-bound, public accountability** an elected
official cannot silently ignore.

| | |
|---|---|
| **Framework** | AGORA — this repository ([agora-fed/core](https://github.com/agora-fed/core)), API and code 100% English |
| **Localization** | `l10n_<cc>` modules — identity documents, territory, voter registry ([ADR-0015](./docs/decisions/ADR-0015-l10n-localization-layer.md)); Brazil: [l10n-brazil](https://github.com/agora-fed/l10n-brazil) |
| **Installation** | A deployment of core + one l10n module + a locale. PINDORAMA = core + l10n-brazil + pt-BR |

The engineering north star is **[PLAN.md](./PLAN.md)**. The Fediverse strategy
(where AGORA is heading as a federated network) is
**[docs/FEDIVERSE-STRATEGY.md](./docs/FEDIVERSE-STRATEGY.md)**.

---

## The core loop

```
propose ─▶ cluster (consensus) ─▶ vote ─▶ threshold ─▶ notify official ─▶ SLA clock ─▶ answered / public silence ─▶ scorecard
                                                                                              │
                                                                                              └─▶ auto-federated ActivityPub Note
```

Four subsystems make this **not** "Decidim in Rust" — they are the point:

| Subsystem | What it does |
|-----------|--------------|
| `consensus`   | Embeds proposals (pgvector), merges duplicates into one real signal |
| `consequence` | Starts a public SLA clock; records answered / acted / **ignored** (write-once) |
| `mandates`    | Indexes officials & candidates (official open data) and onboards them via public e-mail |
| `scorecard`   | Permanent public record: promises vs delivery, answered vs ignored |

On top of the loop, AGORA is a **full ActivityPub citizen network**:
Mastodon-compatible client API (existing apps like Tusky/Elk/Ivory log in via
OAuth), S2S federation with HTTP signatures, notes/polls/media/hashtags,
feeds, forum `Group` actors (FEP-1b12), and an admin console with
Mastodon-grade moderation (reports queue, domain blocks, invites, webhooks).

## Architecture at a glance

- **Language:** Rust (Axum + Tokio), Cargo **workspace of ~23 tiered crates** — the crate
  boundary is the ownership boundary; cross-crate effects flow **only** through the durable
  event log (`events_log`, Postgres) or the gateway (see [PLAN.md §6](./PLAN.md)).
- **DB:** PostgreSQL + `pgvector`, accessed via `sqlx` (compile-checked where cached,
  runtime-checked at the gateway surface — no ORM, auditability is a requirement).
- **Auth:** sovereign e-mail + password (Argon2id); per-country identity documents come
  from the active l10n module ([ADR-0008](./docs/decisions/), [ADR-0015](./docs/decisions/));
  optional national OIDC; session cookie + Mastodon OAuth bearer.
- **Front-end:** Astro SSG + Svelte islands (`web/`), served by the gateway at the same
  origin ([ADR-0009](./docs/decisions/)); PWA with Web Push (RFC 8291).
- **Federation:** ActivityPub S2S + Mastodon client API ([ADR-0005](./docs/decisions/), ADR-0010).
- **Deploy:** GitOps-only ([docs/GITOPS.md](./docs/GITOPS.md)) — Helm chart
  [`deploy/helm/agora-core`](./deploy/helm/agora-core), images from
  [`deploy/docker/`](./deploy/docker/), k3s IPv6-first reference environment
  ([ADR-0002](./docs/decisions/)). Runbooks: [docs/ops/](./docs/ops/).
- **Reliability/audit:** CI on GitHub Actions + a self-hosted Forgejo runner —
  see [docs/CICD.md](./docs/CICD.md).

## Repository layout

See **[docs/PROJECT-STRUCTURE.md](./docs/PROJECT-STRUCTURE.md)** for the full annotated tree.

```
crates/
├── core/  db/  api-contract/        # Tier 0 — frozen contracts (single owner)
├── app/                             # shared AppState (ports: db, clock, storage)
├── platform/                        # Tier 1 — auth, notify, events, consensus,
│                                    #          moderation, admin, l10n-br*
├── gateway/                         # Tier 1 — the ONE public HTTP surface + worker
├── spaces/                          # Tier 2 — processes, assemblies, initiatives,
│                                    #          consultations, mandates (+ parties)
├── components/                      # Tier 2 — proposals, votes, comments, forums,
│                                    #          meetings, budgets, surveys,
│                                    #          accountability, consequence, scorecard
└── clients/                         # Tier 3 — federation SDK (ActivityPub)
web/                                 # Astro + Svelte front-end (SSG → gateway image)
migrations/                          # append-only SQL (GitOps applies them)
deploy/                              # docker/ k8s/ helm/ gitops/
docs/                                # architecture, ADRs, ops runbooks, wiki
scripts/                             # CI guards + data seeds
tests/                               # cross-crate integration harness
```

\* `platform/l10n-br` is being extracted to
[agora-fed/l10n-brazil](https://github.com/agora-fed/l10n-brazil) — the first
standalone localization module.

## Quickstart

### Fast lane (Docker, nothing on the host)

```sh
docker compose -f docker-compose.dev.yml up --build
# gateway on http://localhost:8080 — API under /api/v1, web bundle at /
```

### Developer lane (cargo on the host, DB in Docker)

```sh
docker compose -f docker-compose.dev.yml up -d db
export DATABASE_URL=postgres://dsoc:dsoc@localhost:55432/agora_dev
cargo sqlx migrate run --source migrations

# Guards the CI enforces (run before pushing):
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
./scripts/check-crate-boundaries.sh && ./scripts/check-migration-numbers.sh \
  && ./scripts/check-fk-targets.sh && ./scripts/check-lints-optin.sh

cargo test --workspace --all-features

# Front-end:
cd web && npm install && npm test && npm run build
```

> IPv6-first: every example binds to `[::1]`. IPv4 is an explicit fallback only.

### Production

Production is **GitOps-only**: a release tag builds
`ghcr.io/agora-fed/core:<tag>`, and a commit bumping `image.tag` in the
installation values file is the deploy. No manual `helm`/`kubectl` — ever.
Read **[docs/GITOPS.md](./docs/GITOPS.md)**.

## Documentation

- [PLAN.md](./PLAN.md) — north star (frozen principles, tiers, maturity ladder)
- [docs/FEDIVERSE-STRATEGY.md](./docs/FEDIVERSE-STRATEGY.md) — federation status & wave plan
- [docs/GITOPS.md](./docs/GITOPS.md) — git is the only path to production
- [docs/PROJECT-STRUCTURE.md](./docs/PROJECT-STRUCTURE.md) — annotated repository tree
- [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md) — runtime & crate architecture
- [docs/ROADMAP.md](./docs/ROADMAP.md) — delivery roadmap
- [docs/TESTING.md](./docs/TESTING.md) — test strategy (95% coverage destination, ratcheted)
- [docs/CICD.md](./docs/CICD.md) — pipeline as audit instrument
- [docs/ops/](./docs/ops/) — deployment runbooks (k3s + Helm)
- [docs/decisions/](./docs/decisions/) — ADRs (every reversal justified)
- [CHANGELOG.md](./CHANGELOG.md) — public, versioned

## Language policy

All code, comments, identifiers, commits, and documentation in this repository
are **English** ([ADR-0013](./docs/decisions/ADR-0013-agora-framework-english-api.md)).
Portuguese (or any other language) lives only in:
1. localization modules (`l10n-brazil` carries pt-BR),
2. installation-facing UI copy resolved per locale.

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md). Conventional Commits; `main` is
protected and every change lands through CI.

## The agora-fed organization

- [agora-fed/core](https://github.com/agora-fed/core) — this framework
- [agora-fed/l10n-brazil](https://github.com/agora-fed/l10n-brazil) — Brazilian localization (CPF, voter registry, IBGE)
- future: per-module plugin repositories (`agora-module-*`) once the module
  ABI stabilizes (see docs/FEDIVERSE-STRATEGY.md, wave 2)

# democracia-social

> **Sovereign Participatory Democracy Platform** — codename **PINDORAMA**
> Stewardship: **PopSolutions Cooperativa de Tecnologia** · License: **AGPL-3.0-or-later** (+ [Social Contract](./LICENSE-SOCIAL-CONTRACT.md))
> **Live:** <https://democracia.social.br>

[![CI](https://git.pop.coop/brasil/democracia-social/actions/workflows/ci.yml/badge.svg)](https://git.pop.coop/brasil/democracia-social/actions?workflow=ci.yml)
[![Security](https://git.pop.coop/brasil/democracia-social/actions/workflows/security.yml/badge.svg)](https://git.pop.coop/brasil/democracia-social/actions?workflow=security.yml)
[![Web](https://git.pop.coop/brasil/democracia-social/actions/workflows/web.yml/badge.svg)](https://git.pop.coop/brasil/democracia-social/actions?workflow=web.yml)

Critical democratic infrastructure that connects the entire Brazilian political chain —
vereadores, prefeitos, deputados, senadores, governadores, the Presidency, **and the
candidates for every office** — directly to the population. ~70,000 real mandates
(federal, state and municipal) are indexed from official open data, each with a public
accountability scorecard.

**Thesis:** *Participation without consequence is theater.* This platform converts citizen
demand into **visible, time-bound, public accountability** an elected official cannot
silently ignore.

The full engineering north star is **[PLAN.md](./PLAN.md)**. Read it before writing code.
The product strategy for the 2026 election cycle is
**[docs/PLANO-ESTRATEGICO-2026.md](./docs/PLANO-ESTRATEGICO-2026.md)** (Portuguese).

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

On top of the loop, the platform is a **full ActivityPub citizen network**: Mastodon-compatible
client API (existing apps like Tusky/Elk/Ivory log in via OAuth), S2S federation with HTTP
signatures, notes/polls/media/hashtags, feeds, and an admin console with Mastodon-grade
moderation (reports queue, domain blocks, invites, webhooks).

## Architecture at a glance

- **Language:** Rust (Axum + Tokio), Cargo **workspace of ~23 tiered crates** — the crate
  boundary is the ownership boundary; cross-crate effects flow **only** through the durable
  event log (`events_log`, Postgres) or the gateway (see [PLAN.md §6](./PLAN.md)).
- **DB:** PostgreSQL 17 + `pgvector`, accessed via `sqlx` (compile-checked where cached,
  runtime-checked at the gateway surface — no ORM, auditability is a requirement).
- **Auth:** sovereign e-mail + password (Argon2id) + CPF check-digit validation
  ([ADR-0008](./docs/decisions/)); optional gov.br OIDC (dormant until credentials);
  session cookie + Mastodon OAuth bearer.
- **Front-end:** Astro SSG + Svelte islands (`web/`), served by the gateway at the same
  origin ([ADR-0009](./docs/decisions/)); PWA with Web Push (RFC 8291).
- **Federation:** ActivityPub S2S + Mastodon client API ([ADR-0005](./docs/decisions/), ADR-0010).
- **Deploy:** k3s on a sovereign IPv6-first VM (Caddy TLS front), image built from
  [`deploy/docker/Dockerfile`](./deploy/docker/Dockerfile); Helm chart for the HA future
  ([ADR-0002](./docs/decisions/)). Runbook: [docs/ops/](./docs/ops/).
- **Reliability/audit:** CI/CD on a self-hosted Forgejo runner is the primary trust
  mechanism — see [docs/CICD.md](./docs/CICD.md).

## Repository layout

See **[docs/PROJECT-STRUCTURE.md](./docs/PROJECT-STRUCTURE.md)** for the full annotated tree.

```
crates/
├── core/  db/  api-contract/        # Tier 0 — frozen contracts (single owner)
├── app/                             # shared AppState (ports: db, clock, storage)
├── platform/                        # Tier 1 — auth, notify, events, consensus,
│                                    #          moderation, admin
├── gateway/                         # Tier 1 — the ONE public HTTP surface + worker
├── spaces/                          # Tier 2 — processes, assemblies, initiatives,
│                                    #          consultations, mandates (+ parties)
├── components/                      # Tier 2 — proposals, votes, comments, debates,
│                                    #          meetings, budgets, surveys,
│                                    #          accountability, consequence, scorecard
└── clients/                         # Tier 3 — federation (ActivityPub)
web/                                 # Astro + Svelte front-end (SSG → gateway image)
migrations/                          # append-only SQL, applied manually in prod
deploy/                              # docker/ k8s/ helm/
docs/                                # architecture, ADRs, ops runbooks, wiki
scripts/                             # CI guards + data seeds (Câmara/Senado/TSE)
tests/                               # cross-crate integration harness
```

## Quickstart (developer)

```sh
cp .config/settings.env.example .config/settings.env    # fill in secrets (gitignored)

# Guards the CI enforces (run before pushing):
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
./scripts/check-crate-boundaries.sh && ./scripts/check-migration-numbers.sh \
  && ./scripts/check-fk-targets.sh && ./scripts/check-lints-optin.sh

# Tests need a real PostgreSQL (see docs/TESTING.md):
cargo sqlx migrate run --source migrations
cargo test --workspace --all-features

# Front-end:
cd web && npm install && npm test && npm run build
```

> IPv6-first: every example binds to `[::1]`. IPv4 is an explicit fallback only.

## Documentation

- [PLAN.md](./PLAN.md) — north star (frozen principles, tiers, maturity ladder)
- [docs/PROJECT-STRUCTURE.md](./docs/PROJECT-STRUCTURE.md) — annotated repository tree
- [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md) — runtime & crate architecture
- [docs/PLANO-ESTRATEGICO-2026.md](./docs/PLANO-ESTRATEGICO-2026.md) — product strategy (pt-BR)
- [docs/ROADMAP.md](./docs/ROADMAP.md) — delivery roadmap
- [docs/TESTING.md](./docs/TESTING.md) — test strategy (the reliability backbone)
- [docs/CICD.md](./docs/CICD.md) — pipeline as audit instrument
- [docs/ops/](./docs/ops/) — deployment runbooks (k3s VM + Helm)
- [docs/decisions/](./docs/decisions/) — ADRs (every reversal justified)
- [CHANGELOG.md](./CHANGELOG.md) — public, versioned, cited on /transparencia
- [Wiki](./docs/wiki/Home.md)

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md). All code, comments, commits, and docs are in **English**;
end-user/civic content is in **Portuguese**. Conventional Commits; every commit is pushed to
`main` and deployed in thin, independent slices.

## Companion repositories

- `git.pop.coop/brasil/democracia-social-app` — Flutter mobile app (iOS + Android native), future.

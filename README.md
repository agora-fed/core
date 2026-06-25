# democracia-social

> **Sovereign Participatory Democracy Platform** — codename **PINDORAMA**
> Stewardship: **PopSolutions Cooperativa de Tecnologia** · License: **AGPL-3.0-or-later** (+ Social Contract)

Critical democratic infrastructure that connects the entire Brazilian political chain —
vereadores, prefeitos, deputados, senadores, governadores, the Presidency, **and the
candidates for every office** — directly to the population.

**Thesis:** *Participation without consequence is theater.* This platform converts citizen
demand into **visible, time-bound, public accountability** an elected official cannot
silently ignore.

The full engineering north star is **[PLAN.md](./PLAN.md)**. Read it before writing code.

---

## The core loop (hyperspecialized)

```
propose ─▶ cluster (consensus) ─▶ vote ─▶ notify official ─▶ SLA clock ─▶ answered / public silence
```

Four subsystems make this **not** "Decidim in Rust" — they are the point:

| Subsystem | What it does |
|-----------|--------------|
| `consensus`   | Embeds proposals (pgvector), merges duplicates into one real signal |
| `consequence` | Starts a public SLA clock; records answered / acted / **ignored** |
| `mandates`    | Onboards officials & candidates via their public email |
| `scorecard`   | Permanent public record: promises vs delivery, answered vs ignored |

---

## Architecture at a glance

- **Language:** Rust (Axum + Tokio). **DB:** PostgreSQL 16 + `pgvector`, accessed only via
  `sqlx` compile-time-checked SQL (no ORM, no hidden queries — auditability is a requirement).
- **Auth:** sovereign Zitadel (OIDC). **Cache/realtime:** Redis. **Jobs/events:** Postgres queue (pgmq).
- **Decomposition:** a Cargo **workspace of independent crates**; the crate boundary is the
  agent boundary (see [PLAN.md §6](./PLAN.md)). Cross-crate effects go through events or the gateway only.
- **Deploy:** **Kubernetes + Helm** (IPv6-first) — see [`deploy/helm/`](./deploy/helm) and
  [ADR-0002](./docs/decisions/ADR-0002-kubernetes-helm.md).
- **Reliability/audit:** CI/CD is the primary trust mechanism — see [docs/CICD.md](./docs/CICD.md).

```
crates/
├── core/  db/  api-contract/      # Tier 0 — frozen contracts (single owner)
├── platform/{auth,notify,events,consensus,moderation,admin}   # Tier 1
├── spaces/{processes,assemblies,initiatives,consultations,mandates}  # Tier 2
├── components/{proposals,votes,comments,debates,meetings,
│              budgets,surveys,accountability,consequence,scorecard}  # Tier 2
├── gateway/                       # Tier 1 — public API surface
└── clients/{web,mobile,federation}  # Tier 3
```

## Quickstart (developer)

```sh
cp .config/settings.env.example .config/settings.env   # fill in secrets (gitignored)
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace                                  # needs a real PostgreSQL (see docs/TESTING.md)
```

> IPv6-first: every example binds to `[::1]`. IPv4 is an explicit fallback only.

## Documentation

- [PLAN.md](./PLAN.md) — north star
- [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md)
- [docs/PARALLELIZATION.md](./docs/PARALLELIZATION.md) — how many agents work without colliding
- [docs/TESTING.md](./docs/TESTING.md) — the test strategy (the reliability backbone)
- [docs/CICD.md](./docs/CICD.md) — pipeline as audit instrument
- [docs/ops/DEPLOYMENT.md](./docs/ops/DEPLOYMENT.md) — Kubernetes + Helm
- [docs/decisions/](./docs/decisions/) — ADRs (every reversal justified)
- [Wiki](./docs/wiki/Home.md)

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md). All code, comments, commits, and docs are in **English**;
end-user/civic content is in **Portuguese**.

## Companion repositories

- `git.pop.coop/brasil/democracia-social-app` — Flutter mobile app (iOS + Android native), created later.

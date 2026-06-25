# Architecture

The authoritative source is [PLAN.md](../PLAN.md); this expands the engineering detail.

## 1. Layering & dependency tiers

```
Tier 0  core ─ db ─ api-contract          (frozen contracts, single owner)
            │
Tier 1  auth notify events consensus moderation admin   +  gateway
            │
Tier 2  spaces/*  +  components/*          (one owner agent each)
            │
Tier 3  clients/{web, mobile, federation}  (consume the frozen api-contract)
```

A higher tier may depend on a lower tier; **never** the reverse, and **never** sideways into a
peer crate's internals. The dependency direction is enforced mechanically in CI
(`scripts/check-crate-boundaries.sh`).

## 2. Cross-crate communication

Only three sanctioned channels:

1. **`core` traits** — synchronous service interfaces (and their mocks for testing).
2. **`dsoc-events`** — durable, Postgres-backed pub/sub (pgmq). The event catalog lives in
   `crates/core` (`events` module) and is part of the frozen contract.
3. **`gateway`** — the public HTTP surface that composes handlers from every crate.

No crate calls another crate's functions directly. `proposals` never reaches into `votes`.

## 3. Data layer

- Single PostgreSQL 16 instance (Phases 1–2) with `pgvector`.
- Each crate **owns its tables**; foreign keys across crate boundaries are allowed only to
  `core`-owned identity tables, never to another component's internal tables.
- All access through `sqlx` checked queries. The committed `.sqlx/` offline cache lets CI verify
  queries without a live DB at compile time, while integration tests run against a real Postgres.

## 4. Event flow of the core loop

```
proposals.created
   └─▶ consensus: embed + cluster ──▶ consensus.cluster.formed
   └─▶ moderation: rules + stats   ──▶ moderation.cleared | moderation.flagged
votes.cast ──▶ votes.tally.updated
   └─▶ proposals: threshold check  ──▶ proposals.threshold.crossed
          └─▶ consequence: start SLA ──▶ consequence.sla.started
                 └─▶ notify: push to official (mobile)
                 └─▶ (on expiry) consequence.sla.expired ──▶ scorecard.updated
```

## 5. Identity & privacy

- Sovereign auth via Zitadel (OIDC); `auth` validates tokens and issues sessions with graded
  verification levels.
- Vote privacy: `votes` stores aggregates queryable by officials; per-citizen linkage is minimized
  and protected, never exposed to officials (LGPD; future zero-knowledge track).

## 6. Deployment

Kubernetes + Helm, IPv6-first — see [ops/DEPLOYMENT.md](./ops/DEPLOYMENT.md) and
[ADR-0002](./decisions/ADR-0002-kubernetes-helm.md).

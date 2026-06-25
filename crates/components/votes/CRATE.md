# dsoc-votes

> Tier 2 crate. **One crate = one owner agent.** This file is the crate's
> contract with the rest of the system (PLAN.md section 6.2).

## Responsibility

Votes: support signals on proposals. Stores aggregates queryable by officials; individual voter linkage is minimized/protected (LGPD, DO-NOT in PLAN.md).

## Events

| Direction | Topic |
|-----------|-------|
| emits | `votes.cast` |
| emits | `votes.tally.updated` |

## Owned tables

- `vote`
- `vote_tally`

## Public surface

- Implements the relevant service trait(s) from `dsoc-core`.
- Exposes Axum handlers mounted by `dsoc-gateway`.
- Owns its migrations under `migrations/` (prefixed with the crate slug).

## Boundaries (DO NOT)

- DO NOT import another component crate's internals — cross-crate effects go
  through `dsoc-events` or the gateway.
- DO NOT introduce an ORM or query builder that hides SQL — `sqlx` checked
  queries only.
- DO NOT default any socket/example/doc to IPv4.

## Definition of done

- Domain model + `sqlx` queries + service impl + HTTP handlers + **own tests**.
- `cargo fmt`, `clippy -D warnings`, and `sqlx` checks green against real
  PostgreSQL on the CI runner.
- Per-crate coverage gate met (see docs/TESTING.md).

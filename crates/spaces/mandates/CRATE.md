# dsoc-mandates

> Tier 2 crate. **One crate = one owner agent.** This file is the crate's
> contract with the rest of the system (PLAN.md section 6.2).

## Responsibility

NEW. Mandate & candidate registry: ingests public official/candidate directories (Camara, Senado, prefeituras, TSE), binds each to a public email, drives mandatory onboarding (Decidim failure #6).

## Events

| Direction | Topic |
|-----------|-------|
| emits | `mandates.official.invited` |
| emits | `mandates.official.onboarded` |
| emits | `mandates.identity.verified` |
| consumes | `auth.verification.upgraded` |

## Owned tables

- `mandate`
- `mandate_office`
- `mandate_invitation`
- `mandate_identity_binding`

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

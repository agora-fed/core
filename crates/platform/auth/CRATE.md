# dsoc-auth

> Tier 1 crate. **One crate = one owner agent.** This file is the crate's
> contract with the rest of the system (PLAN.md section 6.2).

## Responsibility

Sovereign identity & verification: Zitadel/OIDC token validation, session issuance, and answering "is this person who they claim?" at graded verification levels.

## Events

| Direction | Topic |
|-----------|-------|
| emits | `auth.verification.upgraded` |
| consumes | `mandates.official.invited` |

> **Drift note (ADR-0004 catalog):** the frozen `dsoc_core::events::Event` catalog has **no
> `auth.session.created` variant**, so session issuance is recorded in `auth_session` rather than
> emitted. Per the wiring rules a crate emits **only** existing catalog variants; if a session
> event is later needed it must be added to the catalog by ADR first. `auth.verification.upgraded`
> is emitted whenever a citizen's level rises.

## Owned tables

- `auth_session` — server-issued sessions. Includes the **ActivityPub-readiness seam** (ADR-0005):
  a stable `public_handle` and a reserved, nullable `actor_public_key` (HTTP-Signatures keypair
  slot; not populated in the MVP). A `auth_session_public` view exposes the handle without the
  OIDC subject.
- `auth_verification_level` — append-only audit trail of verification-level changes.

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

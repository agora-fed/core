# dsoc-admin

> Tier 1 crate. **One crate = one owner agent.** This file is the crate's
> contract with the rest of the system (PLAN.md section 6.2).

## Responsibility

System & organization administration: tenants, roles, feature flags, audit-log surface.

## Events

| Direction | Topic |
|-----------|-------|
| emits | _none_ |
| consumes | _none_ |

The frozen event catalog (`dsoc_core::events::Event`) has **no `admin.*` variants**, and
adding one is a Tier-0 change requiring an ADR (PLAN.md section 5.3). Administration is
internal state management that nothing currently subscribes to, so admin **persists state and
exposes routes but emits no cross-crate events for now**. The `Arc<dyn EventBus>` publish port
is still injected and held by `AdminService` (`AdminService::event_bus`) so a future ADR can wire
emission in `src/events.rs` without changing the service's construction or the gateway.

## Owned tables

- `admin_org` — 1:1 administrative extension of the core-owned `org` (`org_id` PK → `org`).
- `admin_role_binding` — `(id, org_id, citizen_id, role, created_at)`; unique
  `(org_id, citizen_id, role)`; `role ∈ {owner, admin, auditor}`.
- `admin_feature_flag` — `(id, org_id, key, enabled, created_at, updated_at)`; unique
  `(org_id, key)` (the upsert target making toggles idempotent and audited).

Migration: `migrations/0150_admin_core.sql`. Cross-crate FKs target only `org`/`citizen`.

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

## Behaviors under test

Unit (`src/domain.rs`): role string round-trip + unknown-role rejection, role mutate
capability, mutation authorization threshold (directory-level), feature-flag key validation
(empty / too-long / charset / max length), idempotency no-op detection, page-limit clamping.

Integration (`tests/integration.rs`, real Postgres, `RecordingEventBus`, `FixedClock`):

- create org, then bind a role, and read both back (asserts **no events emitted**);
- duplicate role binding → `Conflict`;
- toggling a feature flag is **idempotent and audited** (one row; `created_at` preserved,
  `updated_at` advances under the injected clock);
- unauthorized mutation → `Forbidden` (and nothing persisted).

## Definition of done

- Domain model + `sqlx` queries + service impl + HTTP handlers + **own tests**.
- `cargo fmt`, `clippy -D warnings`, and `sqlx` checks green against real
  PostgreSQL on the CI runner.
- Per-crate coverage gate met (see docs/TESTING.md).

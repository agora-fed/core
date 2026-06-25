# ADR-0004 — Tier-0 contract completion (Wave 0) and re-freeze

- **Status:** Accepted
- **Context:** PLAN.md §5.3 (contract-first; changing frozen Tier-0 needs an ADR) and §6
  (parallel fan-out). Before fanning out one agent per crate, the Tier-0 contract had gaps that every
  downstream agent would otherwise hit. This ADR records the **additive** completions and re-freezes.

## Decisions (all additive — backward-compatible to existing consumers)

1. **`EventBus` publish port** added to `dsoc-core` (`traits.rs`). A crate emits cross-crate events
   through `Arc<dyn EventBus>` instead of depending on `dsoc-events`, preserving the crate boundary.
   `async-trait` keeps it dyn-compatible. A `RecordingEventBus` test double ships in
   `dsoc_core::testing` so every crate can assert emissions without a real bus.
2. **7 event variants + the `Notify` topic** added to the frozen catalog (`events.rs`), reconciling
   the catalog with the CRATE.md contracts: `proposals.published`, `votes.tally.updated`,
   `consensus.proposal.merged`, `mandates.identity.verified`, `auth.verification.upgraded`,
   `notify.dispatched`, `notify.delivery.failed`. Plus the `NotificationId` newtype. `Event` is
   `#[non_exhaustive]`; adding variants is backward-compatible (the catalog's own additive policy).
3. **`AppState` wiring contract** added as the new Tier-0 crate `dsoc-app` (it carries the concrete
   `Db` pool and therefore cannot live in `core`, which must not depend on `db`). Every crate exposes
   `pub fn routes(state: AppState) -> Router<()>`. `dsoc-app` is added to the crate-boundary allowlist.
4. **DTO/OpenAPI strategy:** `api-contract` holds only cross-cutting types (envelope, error,
   pagination). Each crate owns its request/response DTOs + `utoipa` schema fragment; the gateway
   integration step composes them into `/openapi.json`.
5. **Migration discipline:** `migrations/REGISTRY.md` assigns each crate a 10-wide number range; three
   CI guards enforce it — `check-migration-numbers.sh` (no duplicate prefixes),
   `check-fk-targets.sh` (cross-crate FKs only to `org`/`citizen`/`mandate`), and
   `check-lints-optin.sh` (every member opts into workspace lints).
6. **`.sqlx` offline-cache convention:** per-crate caches (`crates/<path>/.sqlx/`) so parallel agents
   don't race one root cache; the integration step runs the authoritative `cargo sqlx prepare --workspace`.

## Consequences

- Re-freeze: `core`, `db`, `api-contract`, and now `app` + the wiring conventions are stable. Further
  changes need a new ADR.
- The crate-owner agent brief (the plan) references these as authoritative.

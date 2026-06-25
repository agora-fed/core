# dsoc-moderation

> Tier 1 crate. **One crate = one owner agent.** This file is the crate's
> contract with the rest of the system (PLAN.md section 6.2).

## Responsibility

Auditable moderation: deterministic rules + statistical anomaly detection + optional local model. No opaque third-party classifiers (principle 11).

## Events

| Direction | Topic |
|-----------|-------|
| emits | `moderation.flagged` |
| emits | `moderation.cleared` |
| consumes | `proposals.created` |
| consumes | `comments.created` |

## Owned tables

- `moderation_rule`
- `moderation_decision`
- `moderation_appeal`

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

## Implemented signal

The "statistical" signal is a fully transparent **uppercase-letter ratio** (shouting
detector), configured per rule as a `caps_ratio` threshold in `0.0..=1.0`. Together with
`keyword` substring rules this gives auditable, reproducible decisions with no opaque
third-party classifier (principle 11). The "optional local model" remains out of scope
for this phase by design.

## Behaviors under test (TESTING.md catalog)

Unit (pure domain, `src/domain.rs`):
- enum/token round-trips (`kind`, `action`, `outcome`, `target_kind`, `status`);
- keyword matcher is case-insensitive and ignores blank patterns;
- uppercase-ratio statistic is exact (no letters ⇒ `0.0`);
- `caps_ratio` rule rejects out-of-range / non-numeric thresholds;
- first-match precedence is oldest-first; empty ruleset always clears;
- appeal state machine: `open -> granted|denied` only; terminal states never flip;
- boundary validation trims and bounds free text.

Integration (`tests/integration.rs`, real Postgres + `RecordingEventBus` + `FixedClock`):
- a matching rule flags, emits `moderation.flagged`, and writes an auditable decision;
- clean content clears and emits `moderation.cleared`;
- with no rules a `cleared` decision is still persisted (never silently dropped);
- consuming `comments.created` audits the comment but keys the event by its proposal;
- an unrelated event is ignored with no decision;
- appeals transition `open -> granted`, and a resolved appeal cannot be re-decided
  (`Conflict`);
- appeal on an unknown decision is `NotFound`; empty reason / bad threshold are
  `Validation`;
- the decision audit log is org-scoped and keyset-paginated newest-first.

## Definition of done

- Domain model + `sqlx` queries + service impl + HTTP handlers + **own tests**.
- `cargo fmt`, `clippy -D warnings`, and `sqlx` checks green against real
  PostgreSQL on the CI runner.
- Per-crate coverage gate met (see docs/TESTING.md).

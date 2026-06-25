# Parallelization — many agents, no collisions

PLAN.md §6, expanded. The goal: maximum concurrent agents with minimal coordination cost.

## Why it works

1. **Frozen contracts (Tier 0).** `core`, `db`, `api-contract` are designed and frozen first by a
   single owner. Everyone else depends on them **read-only**. This is the only sequential bottleneck.
2. **Crate = ownership unit.** One crate, one owner agent, one directory, its own tables and tests.
   No shared files ⇒ no merge hell by construction.
3. **Trait mocks.** A Tier-2/3 agent depending on an unfinished Tier-1 service codes against a mock
   generated from `core`. Nobody blocks on anybody.
4. **Events, not reach-ins.** Cross-crate effects are asynchronous events, so two crates never need
   to edit each other.

## Fan-out schedule

| Tier | Crates | Concurrency | Precondition |
|------|--------|-------------|--------------|
| 0 | core, db, api-contract | 1–2 senior owners (sequential) | — |
| 1 | auth, notify, events, consensus, moderation, admin, gateway | ~7 | Tier 0 frozen |
| 2 | 5 spaces + 10 components | ~15 | Tier 1 contracts exist |
| 3 | web, mobile, federation | ~3 | api-contract frozen (can start early on mocks) |

## Guardrails (mechanical, not review-fatigue)

- `cargo fmt` + `clippy -D warnings` to merge.
- `sqlx` checks against a real Postgres CI runner.
- Per-crate coverage gate.
- `scripts/check-crate-boundaries.sh` fails the build if a component crate path-depends on another
  component crate.
- Conventional Commits, English only. Short-lived per-crate branches.

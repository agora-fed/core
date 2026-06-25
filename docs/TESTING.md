# Testing Strategy — the reliability backbone

> Given the **political weight** of this platform, tests and CI are the primary instrument of
> reliability and audit. A change is not "done" until it is proven by tests running against a
> **real PostgreSQL** (no mocked database, no throwaway behavior).

## Test pyramid

| Layer | Scope | Where | Gate |
|-------|-------|-------|------|
| **Unit** | pure domain logic (IDs, value objects, errors, state machines) | `#[cfg(test)]` in each crate | every crate |
| **Integration** | `sqlx` queries + service impl against real Postgres | `crates/<x>/tests/` | every crate touching the DB |
| **Contract** | DTOs/handlers conform to `api-contract` OpenAPI | `crates/api-contract/tests/` | api surface changes |
| **Event** | emitted/consumed events match the `core` catalog | `crates/core/tests/` + per-crate | event changes |
| **End-to-end** | the core loop: propose → cluster → vote → SLA → silence | `tests/e2e/` (workspace) | release |

## Coverage

- **Minimum 80%** line coverage per crate (PLAN.md / org standard), measured with
  `cargo llvm-cov`. CI fails below threshold.
- The four NEW subsystems (`consequence`, `consensus`, `mandates`, `scorecard`) carry the
  accountability thesis and target **90%+**.

## Golden-path E2E (the thesis test)

`tests/e2e/core_loop.rs` asserts the entire consequence loop, which is the platform's reason to exist:

```
GIVEN an official onboarded via public email (mandates)
WHEN  citizens file similar proposals that cluster (consensus)
AND   support crosses the directed threshold (votes + proposals)
THEN  an SLA clock starts and the official is notified (consequence + notify)
AND   on SLA expiry without response, the outcome is publicly recorded "ignored"
AND   the public scorecard reflects the silence (scorecard)
```

This test must never be skipped or quarantined silently.

## Determinism & isolation

- Each integration test runs in its **own transaction or ephemeral database**, rolled back/dropped
  after. No shared mutable state between tests.
- Time is injected (a `Clock` port in `core`) so SLA-expiry tests are deterministic — never
  `sleep`-based.
- Embeddings in `consensus` tests use a fixed local stub vector set, not a live model.

## Running locally

```sh
# Provision a local PostgreSQL 16 + pgvector, then:
export DATABASE_URL=postgres://dsoc@[::1]:5432/democracia_social_test
cargo sqlx migrate run
cargo test --workspace                 # unit + integration
cargo llvm-cov --workspace --fail-under-lines 80
cargo test -p e2e --test core_loop     # the thesis E2E
```

## Test catalog ("the maximum number of necessary tests")

Every crate's `CRATE.md` enumerates the behaviors under test. New behavior ⇒ new test first
(TDD: RED → GREEN → refactor). The catalog is reviewed in PR; missing tests block merge.

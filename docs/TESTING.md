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

## Coverage: measured baseline and the road to 95%

Destination: **95% line coverage**, enforced two ways (codecov.yml):
- **patch gate, required today**: every new/changed line arrives ≥95% tested;
- **project ratchet**: total coverage may never drop (CI `--fail-under-lines`
  is the measured floor and only goes UP — raising it belongs in the same PR
  that adds coverage).

Measured baseline (2026-08-05, full workspace against a fully migrated
PostgreSQL): **49.8% lines**. Per-crate reality:

| Band | Crates | Read |
|---|---|---|
| 90–97% | `core`, `assemblies`, `consultations`, `events`, `clients/federation`, `scorecard` | already at or near destination |
| 76–90% | most `components/*`, `spaces/*`, `platform/*` | close; targeted tests finish the job |
| 50–55% | `auth` (4.9k lines), `consensus`, `storage` | needs a real test push |
| 43% | `forums` (1.7k lines) | needs a real test push |
| **29%** | **`gateway` (27.5k lines)** | **THE gap — ~19.4k uncovered lines, more than the whole rest of the workspace** |

Conclusion the numbers force: the road to 95% **is** the gateway — and the
strategy is not "write 19k lines of gateway tests". It is the same strategy as
the modularity plan (docs/FEDIVERSE-STRATEGY.md §5.1, wave 2): move logic out
of the gateway into owned, unit-tested crates, leaving the gateway as thin
mounting/translation code, while every extraction lands with tests under the
95% patch gate. Coverage and architecture converge on the same work.

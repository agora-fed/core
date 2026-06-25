# dsoc-consensus

> Tier 1 crate. **One crate = one owner agent.** This file is the crate's
> contract with the rest of the system (PLAN.md section 6.2).

## Responsibility

Semantic clustering & dedupe via pgvector. Embeds every new proposal, finds near-duplicates, and merges noise into one consensus signal (Decidim failure #2).

## Events

| Direction | Topic |
|-----------|-------|
| emits | `consensus.cluster.formed` |
| emits | `consensus.proposal.merged` |
| consumes | `proposals.created` |

## Owned tables

- `consensus_embedding`
- `consensus_cluster`
- `consensus_cluster_member`

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

## Behaviors under test (the test catalog — docs/TESTING.md)

Unit (`src/domain.rs`, pure, deterministic):
- the stub embedder is deterministic, fixed-dimension (384), and unit-norm for non-empty text;
- near-identical texts are within the threshold, dissimilar texts are beyond it;
- cosine distance matches pgvector's `<=>` (zero for identical, `1.0` for a zero vector);
- the threshold decision is inclusive at the boundary and forms just past it;
- empty/blank text is rejected as `Validation`; the pgvector literal formats losslessly.

Integration (`tests/integration.rs`, real PostgreSQL, `FixedClock`, `RecordingEventBus`):
- the first proposal forms a cluster and emits `consensus.cluster.formed`;
- two near-identical texts merge into one cluster and emit `consensus.proposal.merged` (size 2);
- dissimilar texts form separate clusters;
- the cosine-distance threshold boundary is respected (a strict threshold blocks a near merge);
- re-ingesting the same proposal is a `Conflict`; blank text is `Validation` (no event emitted);
- consuming `proposals.created` clusters the proposal; unrelated events are ignored;
- cluster fetch + keyset-paginated listing round-trip; an absent cluster is `NotFound`.

## Definition of done

- Domain model + `sqlx` queries + service impl + HTTP handlers + **own tests**.
- `cargo fmt`, `clippy -D warnings`, and `sqlx` checks green against real
  PostgreSQL on the CI runner.
- Per-crate coverage gate met (see docs/TESTING.md).

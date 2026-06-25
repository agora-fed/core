# Roadmap (engineering)

Mirror of PLAN.md §7; the national political rollout is a separate plan, intentionally out of scope.

## Phase 0 — Foundations & Freeze  *(current)*
- Tier-0 `core`, `db`, `api-contract` designed and **frozen**.
- CI live on the Forgejo runner; fmt/clippy/sqlx/coverage gates enforced.
- Workspace skeleton + `CRATE.md` per crate.
- **Gate:** contracts frozen; an empty `gateway` boots over IPv6 and authenticates via Zitadel.

## Phase 1 — Core accountability loop / MVP
- Tier 1: `auth`, `notify`, `events`, `consensus`, `moderation`, `gateway`.
- Tier 2 (thesis path): `mandates`, `proposals`, `votes`, `comments`, `consequence`, `scorecard`.
- Tier 3: minimal `web` + `mobile` (Flutter) against the frozen contract.
- **Gate (Go/No-Go):** official onboarded by public email; clustered proposal crosses threshold,
  starts an SLA, notifies on mobile, publicly records silence. If not met, shrink scope.

## Phase 2 — Component breadth & regional scale
- Remaining components/spaces; harden moderation (statistical detection at volume) + consensus.
- Scorecard becomes a public, shareable, embeddable artifact.

## Phase 3 — National scale, resilience & federation
- Evaluate CockroachDB for the transactional core only; multi-region; `federation` SDK + hub;
  external security audit; append-only hashed audit log; zero-knowledge vote-privacy track.

# Contributing

> All code, comments, commits, PR descriptions, and documentation are in **English**.
> End-user UI and civic content are in **Portuguese**. (PLAN.md language policy.)

## Golden rules (from PLAN.md)

1. **Contract-first.** Tier-0 crates (`core`, `db`, `api-contract`) are **frozen**. Changing them
   requires an RFC (`docs/decisions/`), because it invalidates parallel work.
2. **One crate = one owner.** Own your crate's directory, tables, and tests. Never edit another
   crate's internals; cross-crate effects go through `dsoc-events` or the gateway.
3. **Explicit SQL only.** `sqlx` compile-time-checked queries. No ORM, no query builder hiding SQL.
4. **IPv6-first.** Never default a socket/example/doc to IPv4.
5. **No secrets in git.** `.config/settings.env` only.
6. **Justify reversals.** Any change of technical direction states (a) why the old fails, (b) whether
   it can be salvaged, (c) why the new is better — recorded as an ADR.

## Workflow

```sh
git switch -c crate/<crate-name>/<short-topic>      # short-lived, trunk-based
# ... implement domain model + sqlx queries + service impl + handlers + tests ...
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
```

Open a PR; CI must be green before merge (see `docs/CICD.md`). PRs are squash-merged.

## Commit format (Conventional Commits)

```
<type>: <description>

<body — why, not just what>
```

Types: `feat`, `fix`, `refactor`, `docs`, `test`, `chore`, `perf`, `ci`.
Example: `feat(consequence): start SLA clock when a cluster crosses the support threshold`.

## Definition of done (per crate)

- [ ] Domain model, `sqlx` queries, service impl, HTTP handlers.
- [ ] Unit + integration tests; coverage gate met (`docs/TESTING.md`).
- [ ] `CRATE.md` updated (responsibility, events emitted/consumed, owned tables).
- [ ] `fmt` + `clippy -D warnings` + `sqlx` checks green against real PostgreSQL.
- [ ] CHANGELOG entry; Decidim concept credited if ported.

## Wiring conventions (frozen by ADR-0004)

- Expose `pub fn routes(state: dsoc_app::AppState) -> axum::Router<()>`. Do not bind sockets in a
  crate (the gateway owns the IPv6 bind).
- Emit events from a domain mutation via the **transactional outbox**
  `dsoc_db::outbox::publish_tx(&mut *tx, &envelope)` so the change and the event commit atomically
  (ADR-0006). Use the injected `Arc<dyn dsoc_core::EventBus>` only for fire-and-forget emission
  outside a transaction. In tests use `dsoc_core::testing::RecordingEventBus`. Never depend on a peer
  `dsoc-*` crate.
- Own your request/response DTOs + `utoipa` fragment in `src/dto.rs`; the gateway composes
  `/openapi.json`. `api-contract` holds only the envelope/error/pagination.
- Migrations live in the shared `migrations/` dir within your assigned range (see
  `migrations/REGISTRY.md`); commit the per-crate `.sqlx/` offline cache (`cargo sqlx prepare`).

## Security & idempotency (ADR-0007)
- Take the acting citizen from the `dsoc_app::CallerId` extractor — NEVER from the request body.
  Authorize with `authz.require(caller.org, caller.citizen, level)`.
- Make consumers idempotent: `dsoc_db::consumed::claim_consumed(&mut *tx, "<crate>", envelope.id)`
  inside the effect transaction, or a naturally-idempotent state guard.

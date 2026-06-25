# Onboarding a new crate-owner agent

You own exactly **one crate**. Read `PLAN.md` §1–3 and §6, then your crate's `CRATE.md`.

1. **Confirm your contract.** Your `CRATE.md` lists your responsibility, the events you emit/consume,
   and the tables you own. These are your only interface to the rest of the system.
2. **Code against Tier-0 only.** Depend on `dsoc-core` (traits, IDs, errors, event catalog) and
   `dsoc-db`. For unfinished Tier-1 services, use the trait mocks from `core` — never block on
   another agent.
3. **Never reach into a peer crate.** Cross-crate effects go through `dsoc-events` or the gateway.
   CI (`scripts/check-crate-boundaries.sh`) will fail the build otherwise.
4. **TDD.** Write the test first. Integration tests run against real PostgreSQL. Coverage ≥ 80%
   (≥ 90% for the NEW subsystems).
5. **Definition of done.** See `CONTRIBUTING.md`. Green fmt/clippy/sqlx/coverage; `CRATE.md` and
   `CHANGELOG.md` updated.

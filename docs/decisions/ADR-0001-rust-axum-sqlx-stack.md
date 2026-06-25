# ADR-0001 — Rust + Axum + sqlx + PostgreSQL/pgvector

- **Status:** Accepted
- **Context:** PLAN.md §5.1 (frozen for MVP).

## Decision

Build the platform in **Rust** (Axum + Tokio), accessing **PostgreSQL 16 + pgvector** exclusively
through **`sqlx`** compile-time-checked queries. No ORM, no query builder that hides SQL.

## Rationale

- **Auditability/sovereignty (principle 3):** anyone can read the code and know exactly what hits
  the database. Decidim's `ActiveRecord` magic (callbacks, STI, polymorphism) is precisely the
  structural debt we refuse to reproduce.
- **Performance (failure #9):** Rust gives low memory footprint and real-time capability under load
  where Rails did not.
- **`pgvector`** keeps semantic clustering (`consensus`) in the same sovereign database — no
  external embedding API for civic content.

## Consequences

- Higher upfront verbosity (explicit SQL, explicit mapping) — accepted as the cost of auditability.
- A committed `.sqlx/` offline cache is required so CI verifies queries without a live DB at
  compile time.

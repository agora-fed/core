//! # dsoc-db — Tier-0 database layer
//!
//! Owns the `sqlx` connection pool, the migration runner, and the project's **query
//! conventions**. There is intentionally no ORM and no query builder that hides SQL
//! (PLAN.md principle 3): every statement is explicit and compile-time-checked by `sqlx`.
//!
//! ## Conventions (binding on every crate)
//! - Tables are owned by exactly one crate and prefixed with its slug (`proposal_*`, …).
//! - Cross-crate foreign keys point only at `core`-owned identity tables.
//! - Timestamps are `timestamptz`, written from the injected [`dsoc_core::Clock`].
//! - All ids are `uuid` (UUIDv7) — see [`dsoc_core::ids`].
//! - Reads that can be unbounded MUST paginate (`LIMIT`/keyset). No `SELECT *` in app code.

pub mod outbox;

use sqlx::postgres::{PgPool, PgPoolOptions};

/// The shared Postgres connection pool type used across the workspace.
pub type Db = PgPool;

/// Re-export so crates pin the same migrator source.
pub use sqlx::migrate;

/// Connect to PostgreSQL using an IPv6-first `DATABASE_URL` (PLAN.md principle 4).
///
/// # Errors
/// Returns the underlying `sqlx::Error` if the pool cannot be established.
pub async fn connect(database_url: &str, max_connections: u32) -> Result<Db, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(max_connections)
        .connect(database_url)
        .await
}

/// Apply all migrations under `/migrations` (run in CI and on boot).
///
/// # Errors
/// Returns a `sqlx::migrate::MigrateError` if a migration fails to apply.
pub async fn run_migrations(pool: &Db) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("../../migrations").run(pool).await
}

#[cfg(test)]
mod tests {
    #[test]
    fn migrator_embeds_the_baseline() {
        // Compile-time check that the migrations directory is wired and embeds files.
        let m = sqlx::migrate!("../../migrations");
        assert!(
            m.migrations.iter().any(|mg| mg.version == 1),
            "baseline migration 0001 must be embedded"
        );
    }
}

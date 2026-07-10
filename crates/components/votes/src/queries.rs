//! Every `sqlx` statement the crate runs, in one place and compile-time checked (PLAN.md
//! principle 3 — explicit, auditable SQL; no ORM, no `SELECT *`, keyset pagination for lists).
//! Functions return raw `sqlx::Error`; [`crate::service`] maps it onto the canonical
//! [`dsoc_core::Error`] model.
//!
//! ## Privacy boundary (LGPD)
//! The **protected linkage** lives in [`insert_vote`] (the only function that touches
//! `votes_vote`, the citizen↔proposal table). The **official-facing aggregate** path —
//! [`tally`], [`list_tallies`], [`count_tallies`] — reads only `votes_vote_tally` and selects no
//! citizen column. [`crate::service`]'s official methods import *only* the aggregate functions, so
//! a citizen id structurally cannot reach an official response.

use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

/// A privacy-safe aggregate row (`votes_vote_tally`). Carries no citizen linkage.
#[derive(Debug, Clone)]
pub(crate) struct TallyRow {
    pub proposal_id: Uuid,
    pub support_count: i64,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// PROTECTED mutation path — touches the citizen↔proposal linkage (`votes_vote`).
// ---------------------------------------------------------------------------

/// Record a citizen's support signal for a proposal. Idempotent at the row level via
/// `ON CONFLICT (proposal_id, citizen_id) DO NOTHING`: a first cast inserts and returns the **real
/// stored id**; a duplicate cast inserts nothing and returns `None`, leaving the tally untouched
/// (the caller maps `None` to [`dsoc_core::Error::Conflict`]). Returning the id avoids surfacing a
/// phantom — the value handed back is exactly the row PostgreSQL persisted.
pub(crate) async fn insert_vote<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    org_id: Uuid,
    proposal_id: Uuid,
    citizen_id: Uuid,
    created_at: DateTime<Utc>,
) -> Result<Option<Uuid>, sqlx::Error> {
    let stored = sqlx::query_scalar!(
        r#"
        INSERT INTO votes_vote (id, org_id, proposal_id, citizen_id, created_at)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (proposal_id, citizen_id) DO NOTHING
        RETURNING id
        "#,
        id,
        org_id,
        proposal_id,
        citizen_id,
        created_at,
    )
    .fetch_optional(exec)
    .await?;
    Ok(stored)
}

/// Increment a proposal's aggregate support count by one, creating the tally row on first support.
/// Returns the **new** count so the caller can emit `votes.tally.updated` without a re-select. Runs
/// in the same transaction as [`insert_vote`], so the linkage write and the aggregate increment
/// commit atomically.
pub(crate) async fn upsert_tally<'e, E: PgExecutor<'e>>(
    exec: E,
    proposal_id: Uuid,
    updated_at: DateTime<Utc>,
) -> Result<i64, sqlx::Error> {
    let count = sqlx::query_scalar!(
        r#"
        INSERT INTO votes_vote_tally (proposal_id, support_count, updated_at)
        VALUES ($1, 1, $2)
        ON CONFLICT (proposal_id) DO UPDATE
            SET support_count = votes_vote_tally.support_count + 1,
                updated_at = $2
        RETURNING support_count
        "#,
        proposal_id,
        updated_at,
    )
    .fetch_one(exec)
    .await?;
    Ok(count)
}

// ---------------------------------------------------------------------------
// OFFICIAL-FACING aggregate path — reads ONLY `votes_vote_tally`, never `votes_vote`.
// Selecting an explicit column list (never `SELECT *`) makes the absence of any citizen
// column auditable at a glance.
// ---------------------------------------------------------------------------

/// Read the privacy-safe aggregate for one proposal. Bounded by the primary key.
pub(crate) async fn tally<'e, E: PgExecutor<'e>>(
    exec: E,
    proposal_id: Uuid,
) -> Result<Option<TallyRow>, sqlx::Error> {
    let row = sqlx::query_as!(
        TallyRow,
        r#"
        SELECT proposal_id, support_count, updated_at
        FROM votes_vote_tally
        WHERE proposal_id = $1
        "#,
        proposal_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

/// Keyset-paginated browse of aggregates, ascending by `proposal_id` (UUIDv7 ⇒ creation order).
/// `after` is the last `proposal_id` of the previous page; pass `None` for the first page. Bounded
/// by `limit` (the caller clamps it) — no unbounded read.
pub(crate) async fn list_tallies<'e, E: PgExecutor<'e>>(
    exec: E,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<TallyRow>, sqlx::Error> {
    let rows = sqlx::query_as!(
        TallyRow,
        r#"
        SELECT proposal_id, support_count, updated_at
        FROM votes_vote_tally
        WHERE ($1::uuid IS NULL OR proposal_id > $1)
        ORDER BY proposal_id ASC
        LIMIT $2
        "#,
        after,
        limit,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Cross-table reads pra o gate de voto urgente (0.25.0-fediverso — P4.3).
//
// Exceção intencional ao padrão "cada crate lê só suas tabelas": o gate
// depende de `proposal.urgencia` (owner: dsoc-proposals) + `citizen.titulo_status`
// (owner: dsoc-auth). Ambos são **facts** identity-tier — mesma justificativa
// que já permite votes ler `citizen.verification_level` via `authz.require`.
// O check-crate-boundaries.sh valida deps do Cargo, não SQL, então isto passa.
// ---------------------------------------------------------------------------

/// Lê `proposal.urgencia`. `None` quando o proposal id é inválido.
pub(crate) async fn read_proposal_urgencia<'e, E: PgExecutor<'e>>(
    exec: E,
    proposal_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query_scalar!("SELECT urgencia FROM proposal WHERE id = $1", proposal_id,)
        .fetch_optional(exec)
        .await?;
    Ok(row)
}

/// Lê `citizen.titulo_status`. `None` quando o cidadão nunca vinculou título.
pub(crate) async fn read_citizen_titulo_status<'e, E: PgExecutor<'e>>(
    exec: E,
    citizen_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query_scalar!(
        "SELECT titulo_status FROM citizen WHERE id = $1",
        citizen_id,
    )
    .fetch_optional(exec)
    .await?
    .flatten();
    Ok(row)
}

/// Count all aggregate rows (for list pagination metadata). Aggregate-only; no citizen exposure.
pub(crate) async fn count_tallies<'e, E: PgExecutor<'e>>(exec: E) -> Result<i64, sqlx::Error> {
    let total = sqlx::query_scalar!(
        r#"
        SELECT count(*) AS "count!"
        FROM votes_vote_tally
        "#,
    )
    .fetch_one(exec)
    .await?;
    Ok(total)
}

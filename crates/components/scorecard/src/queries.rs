//! Every database statement for the scorecard projection, as `sqlx` compile-time-checked queries
//! (PLAN.md principle 3 — no ORM, no `SELECT *`, keyset pagination on unbounded reads).
//!
//! Functions return domain types from [`crate::domain`]. Text-coded enums are decoded here; a decode
//! failure means corrupt storage and surfaces as [`Error::Decode`]. The service layer maps [`Error`]
//! onto the canonical [`dsoc_core::Error`].
//!
//! Write paths that participate in the transactional outbox (ADR-0006) are generic over
//! `sqlx::PgExecutor`, so the service can pass `&mut *tx` to make them part of an open transaction.

use chrono::{DateTime, Utc};
use dsoc_db::Db;
use uuid::Uuid;

use crate::domain::{Entry, Outcome, ParseError, Promise, Scorecard};

/// A storage-layer failure: a `sqlx` error or a value that does not decode into the domain
/// (a corrupt row, which the table `CHECK` constraints should prevent).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The underlying database call failed.
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    /// A stored value could not be decoded into the domain model.
    #[error(transparent)]
    Decode(#[from] ParseError),
}

/// Keyset cursor for newest-first pagination: rows strictly older than `(at, id)`.
#[derive(Debug, Clone, Copy)]
pub struct Cursor {
    /// The ordering timestamp of the last row from the previous page.
    pub at: DateTime<Utc>,
    /// The `id` of the last row from the previous page (tie-breaker).
    pub id: Uuid,
}

fn split_cursor(cursor: Option<Cursor>) -> (Option<DateTime<Utc>>, Option<Uuid>) {
    match cursor {
        Some(c) => (Some(c.at), Some(c.id)),
        None => (None, None),
    }
}

// --- row shapes (kept private; decoded into domain via the helpers below) -----------

struct ScorecardRow {
    id: Uuid,
    org_id: Uuid,
    mandate_id: Uuid,
    answered: i64,
    ignored: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<ScorecardRow> for Scorecard {
    fn from(r: ScorecardRow) -> Self {
        Self {
            id: r.id,
            org_id: r.org_id,
            mandate_id: r.mandate_id,
            answered: r.answered,
            ignored: r.ignored,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

struct EntryRow {
    id: Uuid,
    scorecard_id: Uuid,
    sla_id: Uuid,
    outcome: String,
    response_hours: Option<f64>,
    occurred_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
}

impl EntryRow {
    fn into_domain(self) -> Result<Entry, ParseError> {
        Ok(Entry {
            id: self.id,
            scorecard_id: self.scorecard_id,
            sla_id: self.sla_id,
            outcome: self.outcome.parse::<Outcome>()?,
            response_hours: self.response_hours,
            occurred_at: self.occurred_at,
            created_at: self.created_at,
        })
    }
}

struct PromiseRow {
    id: Uuid,
    scorecard_id: Uuid,
    text: String,
    made_at: DateTime<Utc>,
    delivered: bool,
    delivered_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

impl From<PromiseRow> for Promise {
    fn from(r: PromiseRow) -> Self {
        Self {
            id: r.id,
            scorecard_id: r.scorecard_id,
            text: r.text,
            made_at: r.made_at,
            delivered: r.delivered,
            delivered_at: r.delivered_at,
            created_at: r.created_at,
        }
    }
}

// --- scorecard (the projection root) ------------------------------------------------

/// Get-or-create the scorecard for a mandate and return its REAL stored id. The candidate `id`
/// is used only when inserting; on conflict the existing row's id is returned (never a phantom).
/// The `ON CONFLICT ... DO UPDATE` no-op write guarantees `RETURNING` fires on the conflict path.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn upsert_scorecard(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    org_id: Uuid,
    mandate_id: Uuid,
    now: DateTime<Utc>,
) -> Result<Uuid, Error> {
    let row = sqlx::query!(
        r#"
        INSERT INTO scorecard (id, org_id, mandate_id, answered, ignored, created_at, updated_at)
        VALUES ($1, $2, $3, 0, 0, $4, $4)
        ON CONFLICT (mandate_id) DO UPDATE SET mandate_id = EXCLUDED.mandate_id
        RETURNING id
        "#,
        id,
        org_id,
        mandate_id,
        now,
    )
    .fetch_one(executor)
    .await?;
    Ok(row.id)
}

/// Overwrite a scorecard's counters with a fresh recount (the invariant: counters always equal a
/// recount of the ledger) and stamp `updated_at`.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn update_counters(
    executor: impl sqlx::PgExecutor<'_>,
    scorecard_id: Uuid,
    answered: i64,
    ignored: i64,
    now: DateTime<Utc>,
) -> Result<(), Error> {
    sqlx::query!(
        r#"UPDATE scorecard SET answered = $2, ignored = $3, updated_at = $4 WHERE id = $1"#,
        scorecard_id,
        answered,
        ignored,
        now,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// Fetch a scorecard by id.
///
/// # Errors
/// [`sqlx::Error::RowNotFound`] (wrapped in [`Error::Db`]) when absent.
pub async fn get_scorecard(db: &Db, id: Uuid) -> Result<Scorecard, Error> {
    let row = sqlx::query_as!(
        ScorecardRow,
        r#"
        SELECT id, org_id, mandate_id, answered, ignored, created_at, updated_at
        FROM scorecard
        WHERE id = $1
        "#,
        id,
    )
    .fetch_one(db)
    .await?;
    Ok(row.into())
}

/// Fetch a scorecard by mandate, or `None` if the politician has none yet.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn get_scorecard_by_mandate(
    db: &Db,
    mandate_id: Uuid,
) -> Result<Option<Scorecard>, Error> {
    let row = sqlx::query_as!(
        ScorecardRow,
        r#"
        SELECT id, org_id, mandate_id, answered, ignored, created_at, updated_at
        FROM scorecard
        WHERE mandate_id = $1
        "#,
        mandate_id,
    )
    .fetch_optional(db)
    .await?;
    Ok(row.map(Into::into))
}

/// List an org's scorecards newest-first with keyset pagination.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn list_scorecards(
    db: &Db,
    org_id: Uuid,
    cursor: Option<Cursor>,
    limit: i64,
) -> Result<Vec<Scorecard>, Error> {
    let (cursor_at, cursor_id) = split_cursor(cursor);
    let rows = sqlx::query_as!(
        ScorecardRow,
        r#"
        SELECT id, org_id, mandate_id, answered, ignored, created_at, updated_at
        FROM scorecard
        WHERE org_id = $1
          AND ($2::timestamptz IS NULL OR (created_at, id) < ($2, $3::uuid))
        ORDER BY created_at DESC, id DESC
        LIMIT $4
        "#,
        org_id,
        cursor_at,
        cursor_id,
        limit,
    )
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

// --- scorecard_entry (the event-sourced ledger) -------------------------------------

/// Append a ledger entry, idempotently. Returns `Some(id)` when a new row was written, or `None`
/// when the `sla_id` was already present (an at-least-once replay — ADR-0006): the duplicate is
/// dropped, so the counters never double-count.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
#[allow(clippy::too_many_arguments)]
pub async fn insert_entry(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    scorecard_id: Uuid,
    sla_id: Uuid,
    outcome: Outcome,
    response_hours: Option<f64>,
    occurred_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<Option<Uuid>, Error> {
    let row = sqlx::query!(
        r#"
        INSERT INTO scorecard_entry
            (id, scorecard_id, sla_id, outcome, response_hours, occurred_at, created_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        ON CONFLICT (sla_id) DO NOTHING
        RETURNING id
        "#,
        id,
        scorecard_id,
        sla_id,
        outcome.as_str(),
        response_hours,
        occurred_at,
        now,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row.map(|r| r.id))
}

/// Recount a scorecard's counters directly from the ledger (the source of truth). Returns
/// `(answered, ignored)`.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn recount_entries(
    executor: impl sqlx::PgExecutor<'_>,
    scorecard_id: Uuid,
) -> Result<(i64, i64), Error> {
    let row = sqlx::query!(
        r#"
        SELECT
            COALESCE(count(*) FILTER (WHERE outcome = 'answered'), 0) AS "answered!",
            COALESCE(count(*) FILTER (WHERE outcome = 'ignored'), 0)  AS "ignored!"
        FROM scorecard_entry
        WHERE scorecard_id = $1
        "#,
        scorecard_id,
    )
    .fetch_one(executor)
    .await?;
    Ok((row.answered, row.ignored))
}

/// The response latencies (hours) of all answered entries for a scorecard, oldest-first. Feeds the
/// pure median computation in [`crate::domain::median_hours`].
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn answered_response_hours(db: &Db, scorecard_id: Uuid) -> Result<Vec<f64>, Error> {
    let rows = sqlx::query!(
        r#"
        SELECT response_hours AS "response_hours!"
        FROM scorecard_entry
        WHERE scorecard_id = $1 AND outcome = 'answered'
        ORDER BY occurred_at ASC, id ASC
        "#,
        scorecard_id,
    )
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(|r| r.response_hours).collect())
}

/// Fetch a scorecard's full ledger, oldest-first (bounded by `limit`). Used by tests to assert the
/// recount invariant and by audit reads.
///
/// # Errors
/// [`Error::Db`] on a storage failure; [`Error::Decode`] on a corrupt row.
pub async fn list_entries(db: &Db, scorecard_id: Uuid, limit: i64) -> Result<Vec<Entry>, Error> {
    let rows = sqlx::query_as!(
        EntryRow,
        r#"
        SELECT id, scorecard_id, sla_id, outcome, response_hours, occurred_at, created_at
        FROM scorecard_entry
        WHERE scorecard_id = $1
        ORDER BY occurred_at ASC, id ASC
        LIMIT $2
        "#,
        scorecard_id,
        limit,
    )
    .fetch_all(db)
    .await?;
    rows.into_iter()
        .map(EntryRow::into_domain)
        .collect::<Result<_, _>>()
        .map_err(Error::from)
}

// --- scorecard_promise (promises vs delivery) ---------------------------------------

/// Persist a new promise in the undelivered state.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn insert_promise(db: &Db, promise: &Promise) -> Result<(), Error> {
    sqlx::query!(
        r#"
        INSERT INTO scorecard_promise
            (id, scorecard_id, text, made_at, delivered, delivered_at, created_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
        promise.id,
        promise.scorecard_id,
        promise.text,
        promise.made_at,
        promise.delivered,
        promise.delivered_at,
        promise.created_at,
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Fetch a promise by id.
///
/// # Errors
/// [`sqlx::Error::RowNotFound`] (wrapped in [`Error::Db`]) when absent.
pub async fn get_promise(db: &Db, id: Uuid) -> Result<Promise, Error> {
    let row = sqlx::query_as!(
        PromiseRow,
        r#"
        SELECT id, scorecard_id, text, made_at, delivered, delivered_at, created_at
        FROM scorecard_promise
        WHERE id = $1
        "#,
        id,
    )
    .fetch_one(db)
    .await?;
    Ok(row.into())
}

/// Mark a promise delivered, guarding the transition with the expected prior state
/// (`delivered = false`) to close the TOCTOU race. Returns `Some(promise)` when this call flipped
/// it, or `None` when it was already delivered (idempotent — the caller re-reads the row).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn mark_promise_delivered(
    db: &Db,
    id: Uuid,
    delivered_at: DateTime<Utc>,
) -> Result<Option<Promise>, Error> {
    let row = sqlx::query_as!(
        PromiseRow,
        r#"
        UPDATE scorecard_promise
        SET delivered = true, delivered_at = $2
        WHERE id = $1 AND delivered = false
        RETURNING id, scorecard_id, text, made_at, delivered, delivered_at, created_at
        "#,
        id,
        delivered_at,
    )
    .fetch_optional(db)
    .await?;
    Ok(row.map(Into::into))
}

/// List a scorecard's promises newest-first with keyset pagination.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn list_promises(
    db: &Db,
    scorecard_id: Uuid,
    cursor: Option<Cursor>,
    limit: i64,
) -> Result<Vec<Promise>, Error> {
    let (cursor_at, cursor_id) = split_cursor(cursor);
    let rows = sqlx::query_as!(
        PromiseRow,
        r#"
        SELECT id, scorecard_id, text, made_at, delivered, delivered_at, created_at
        FROM scorecard_promise
        WHERE scorecard_id = $1
          AND ($2::timestamptz IS NULL OR (made_at, id) < ($2, $3::uuid))
        ORDER BY made_at DESC, id DESC
        LIMIT $4
        "#,
        scorecard_id,
        cursor_at,
        cursor_id,
        limit,
    )
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

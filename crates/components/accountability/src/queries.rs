//! Every database statement for accountability, as `sqlx` compile-time-checked queries
//! (PLAN.md principle 3 — no ORM, no `SELECT *`, keyset pagination on unbounded reads).
//!
//! Functions return domain types from [`crate::domain`]. The text-coded [`crate::domain::Status`]
//! is decoded here; a decode failure means corrupt storage and surfaces as [`Error::Decode`]. The
//! service layer maps [`Error`] onto the canonical [`dsoc_core::Error`].
//!
//! The status-update write path is generic over `sqlx::PgExecutor`, so the service can pass
//! `&mut *tx` to make the ledger insert and the parent recompute part of ONE transaction.

use chrono::{DateTime, Utc};
use dsoc_db::Db;
use uuid::Uuid;

use crate::domain::{Commitment, ParseError, Status, StatusUpdate};

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

struct ResultRow {
    id: Uuid,
    org_id: Uuid,
    title: String,
    target: String,
    status: String,
    progress: i32,
    created_at: DateTime<Utc>,
}

impl ResultRow {
    fn into_domain(self) -> std::result::Result<Commitment, ParseError> {
        Ok(Commitment {
            id: self.id,
            org_id: self.org_id,
            title: self.title,
            target: self.target,
            status: self.status.parse::<Status>()?,
            progress: self.progress,
            created_at: self.created_at,
        })
    }
}

struct UpdateRow {
    id: Uuid,
    result_id: Uuid,
    note: String,
    progress: i32,
    created_at: DateTime<Utc>,
}

impl From<UpdateRow> for StatusUpdate {
    fn from(r: UpdateRow) -> Self {
        Self {
            id: r.id,
            result_id: r.result_id,
            note: r.note,
            progress: r.progress,
            created_at: r.created_at,
        }
    }
}

// --- accountability_result ----------------------------------------------------------

/// Insert a new result in its initial (`pending`, progress 0) state and return its REAL stored id.
///
/// # Errors
/// Propagates the underlying `sqlx::Error` (e.g. FK/`CHECK` violations).
pub async fn insert_result(
    db: &Db,
    id: Uuid,
    org_id: Uuid,
    title: &str,
    target: &str,
    now: DateTime<Utc>,
) -> std::result::Result<Uuid, Error> {
    let row = sqlx::query!(
        r#"
        INSERT INTO accountability_result (id, org_id, title, target, status, progress, created_at)
        VALUES ($1, $2, $3, $4, 'pending', 0, $5)
        RETURNING id
        "#,
        id,
        org_id,
        title,
        target,
        now,
    )
    .fetch_one(db)
    .await?;
    Ok(row.id)
}

/// Fetch a result by id.
///
/// # Errors
/// [`sqlx::Error::RowNotFound`] (wrapped in [`Error::Db`]) when absent; [`Error::Decode`] on a
/// corrupt stored status.
pub async fn get_result(db: &Db, id: Uuid) -> std::result::Result<Commitment, Error> {
    let row = sqlx::query_as!(
        ResultRow,
        r#"
        SELECT id, org_id, title, target, status, progress, created_at
        FROM accountability_result
        WHERE id = $1
        "#,
        id,
    )
    .fetch_one(db)
    .await?;
    Ok(row.into_domain()?)
}

/// Apply a guarded progress transition: set `progress`/`status` only while the result is NOT yet
/// terminal (`status <> 'achieved'`), stamping the derived status. Returns `Some(result)` when the
/// guard held and the row moved, or `None` when the result was already terminal (the caller maps
/// that to a conflict). The guard closes the TOCTOU race against a concurrent update.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`; [`Error::Decode`] on a corrupt stored status.
pub async fn apply_progress(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    progress: i32,
    status: Status,
) -> std::result::Result<Option<Commitment>, Error> {
    let row = sqlx::query_as!(
        ResultRow,
        r#"
        UPDATE accountability_result
        SET progress = $2, status = $3
        WHERE id = $1 AND status <> 'achieved'
        RETURNING id, org_id, title, target, status, progress, created_at
        "#,
        id,
        progress,
        status.as_str(),
    )
    .fetch_optional(executor)
    .await?;
    row.map(ResultRow::into_domain)
        .transpose()
        .map_err(Error::from)
}

/// List an org's results newest-first with keyset pagination.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`; [`Error::Decode`] on a corrupt stored status.
pub async fn list_results(
    db: &Db,
    org_id: Uuid,
    cursor: Option<Cursor>,
    limit: i64,
) -> std::result::Result<Vec<Commitment>, Error> {
    let (cursor_at, cursor_id) = split_cursor(cursor);
    let rows = sqlx::query_as!(
        ResultRow,
        r#"
        SELECT id, org_id, title, target, status, progress, created_at
        FROM accountability_result
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
    rows.into_iter()
        .map(ResultRow::into_domain)
        .collect::<std::result::Result<_, _>>()
        .map_err(Error::from)
}

// --- accountability_status_update ---------------------------------------------------

/// Append a status-update row to the ledger (part of the open transaction).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn insert_status_update(
    executor: impl sqlx::PgExecutor<'_>,
    update: &StatusUpdate,
) -> std::result::Result<(), Error> {
    sqlx::query!(
        r#"
        INSERT INTO accountability_status_update (id, result_id, note, progress, created_at)
        VALUES ($1, $2, $3, $4, $5)
        "#,
        update.id,
        update.result_id,
        update.note,
        update.progress,
        update.created_at,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// List a result's status updates newest-first with keyset pagination.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn list_status_updates(
    db: &Db,
    result_id: Uuid,
    cursor: Option<Cursor>,
    limit: i64,
) -> std::result::Result<Vec<StatusUpdate>, Error> {
    let (cursor_at, cursor_id) = split_cursor(cursor);
    let rows = sqlx::query_as!(
        UpdateRow,
        r#"
        SELECT id, result_id, note, progress, created_at
        FROM accountability_status_update
        WHERE result_id = $1
          AND ($2::timestamptz IS NULL OR (created_at, id) < ($2, $3::uuid))
        ORDER BY created_at DESC, id DESC
        LIMIT $4
        "#,
        result_id,
        cursor_at,
        cursor_id,
        limit,
    )
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

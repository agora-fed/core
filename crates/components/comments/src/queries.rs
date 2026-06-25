//! Every database statement for comments, as `sqlx` compile-time-checked queries
//! (PLAN.md principle 3 — no ORM, no `SELECT *`, keyset pagination on unbounded reads).
//!
//! Functions that participate in a domain transaction take a generic `PgExecutor` so the
//! caller can pass `&mut *tx` and have the write and the outbox emission commit atomically
//! (ADR-0006). Read-only helpers take `&Db`. Text-coded enums are decoded into the domain
//! here; a decode failure means corrupt storage and surfaces as [`Error::Decode`].

use chrono::{DateTime, Utc};
use dsoc_db::Db;
use sqlx::PgExecutor;
use uuid::Uuid;

use crate::domain::{Comment, CommentStatus, CommentVote, ParseError};

/// A storage-layer failure: a `sqlx` error or a value that does not decode into the
/// domain (a corrupt row, which the `CHECK` constraints should prevent).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The underlying database call failed.
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    /// A stored value could not be decoded into the domain model.
    #[error(transparent)]
    Decode(#[from] ParseError),
}

/// Keyset cursor for oldest-first thread pagination: rows strictly after `(at, id)`.
#[derive(Debug, Clone, Copy)]
pub struct Cursor {
    /// The `created_at` of the last row from the previous page.
    pub at: DateTime<Utc>,
    /// The `id` of the last row from the previous page (tie-breaker).
    pub id: Uuid,
}

/// The minimal projection of a parent comment needed to place a reply: its depth (to
/// compute the child's depth) and the proposal it belongs to (so a reply cannot jump
/// threads). Fetched `FOR UPDATE` inside the insert transaction to close the TOCTOU race
/// between "does the parent exist?" and "insert the child".
#[derive(Debug, Clone, Copy)]
pub struct ParentRef {
    /// The parent's nesting depth.
    pub depth: i32,
    /// The proposal the parent (and therefore the reply) belongs to.
    pub proposal_id: Uuid,
}

// --- row shapes (kept private; decoded into the domain via the helpers below) --------

struct CommentRow {
    id: Uuid,
    org_id: Uuid,
    proposal_id: Uuid,
    parent_id: Option<Uuid>,
    author_id: Uuid,
    body: String,
    depth: i32,
    status: String,
    created_at: DateTime<Utc>,
}

impl CommentRow {
    fn into_domain(self) -> Result<Comment, ParseError> {
        Ok(Comment {
            id: self.id,
            org_id: self.org_id,
            proposal_id: self.proposal_id,
            parent_id: self.parent_id,
            author_id: self.author_id,
            body: self.body,
            depth: self.depth,
            status: self.status.parse::<CommentStatus>()?,
            created_at: self.created_at,
        })
    }
}

struct VoteRow {
    id: Uuid,
    comment_id: Uuid,
    citizen_id: Uuid,
    weight: i16,
    created_at: DateTime<Utc>,
}

impl From<VoteRow> for CommentVote {
    fn from(r: VoteRow) -> Self {
        Self {
            id: r.id,
            comment_id: r.comment_id,
            citizen_id: r.citizen_id,
            weight: r.weight,
            created_at: r.created_at,
        }
    }
}

// --- comments ------------------------------------------------------------------------

/// Lock and project a parent comment for placing a reply. `FOR UPDATE` serializes
/// concurrent replies under the same parent so the depth read cannot go stale before the
/// child insert commits.
///
/// # Errors
/// [`Error::Db`] on a storage failure. Absence is reported as `Ok(None)` so the service
/// can translate a missing parent into a clean [`dsoc_core::Error::Validation`].
pub async fn lock_parent<'e, E>(executor: E, parent_id: Uuid) -> Result<Option<ParentRef>, Error>
where
    E: PgExecutor<'e>,
{
    let row = sqlx::query!(
        r#"
        SELECT depth, proposal_id
        FROM comment
        WHERE id = $1
        FOR UPDATE
        "#,
        parent_id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row.map(|r| ParentRef {
        depth: r.depth,
        proposal_id: r.proposal_id,
    }))
}

/// Persist a new comment node. Runs on the caller's executor so it commits atomically with
/// the outbox emission. The caller supplies the id, depth, status, and timestamp.
///
/// # Errors
/// [`Error::Db`] on a storage failure (e.g. unknown `org_id`/`author_id`, or a `parent_id`
/// that no longer exists).
pub async fn insert_comment<'e, E>(executor: E, comment: &Comment) -> Result<(), Error>
where
    E: PgExecutor<'e>,
{
    sqlx::query!(
        r#"
        INSERT INTO comment
            (id, org_id, proposal_id, parent_id, author_id, body, depth, status, created_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
        comment.id,
        comment.org_id,
        comment.proposal_id,
        comment.parent_id,
        comment.author_id,
        comment.body,
        comment.depth,
        comment.status.as_str(),
        comment.created_at,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// Fetch a single comment by id.
///
/// # Errors
/// [`sqlx::Error::RowNotFound`] (wrapped in [`Error::Db`]) when absent; [`Error::Decode`]
/// on a corrupt row.
pub async fn get_comment(db: &Db, id: Uuid) -> Result<Comment, Error> {
    let row = sqlx::query_as!(
        CommentRow,
        r#"
        SELECT id, org_id, proposal_id, parent_id, author_id, body, depth, status, created_at
        FROM comment
        WHERE id = $1
        "#,
        id,
    )
    .fetch_one(db)
    .await?;
    Ok(row.into_domain()?)
}

/// List a proposal's thread oldest-first with keyset pagination, so a reply always follows
/// its parent within a page. Bounded by `limit` (PLAN.md: unbounded reads must paginate).
///
/// # Errors
/// [`Error::Db`] on a storage failure; [`Error::Decode`] on a corrupt row.
pub async fn list_thread(
    db: &Db,
    org_id: Uuid,
    proposal_id: Uuid,
    cursor: Option<Cursor>,
    limit: i64,
) -> Result<Vec<Comment>, Error> {
    let (cursor_at, cursor_id) = split_cursor(cursor);
    let rows = sqlx::query_as!(
        CommentRow,
        r#"
        SELECT id, org_id, proposal_id, parent_id, author_id, body, depth, status, created_at
        FROM comment
        WHERE org_id = $1
          AND proposal_id = $2
          AND ($3::timestamptz IS NULL OR (created_at, id) > ($3, $4::uuid))
        ORDER BY created_at ASC, id ASC
        LIMIT $5
        "#,
        org_id,
        proposal_id,
        cursor_at,
        cursor_id,
        limit,
    )
    .fetch_all(db)
    .await?;
    rows.into_iter()
        .map(CommentRow::into_domain)
        .collect::<Result<_, _>>()
        .map_err(Error::from)
}

/// Flag every still-`visible` comment of a proposal, returning the number transitioned.
/// The `status = 'visible'` guard makes this an optimistic state transition: a redundant
/// moderation delivery flags zero rows (already-in-target), so the consumer is idempotent.
///
/// # Errors
/// [`Error::Db`] on a storage failure.
pub async fn flag_visible_comments<'e, E>(
    executor: E,
    org_id: Uuid,
    proposal_id: Uuid,
) -> Result<u64, Error>
where
    E: PgExecutor<'e>,
{
    let result = sqlx::query!(
        r#"
        UPDATE comment
        SET status = 'flagged'
        WHERE org_id = $1
          AND proposal_id = $2
          AND status = 'visible'
        "#,
        org_id,
        proposal_id,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

// --- votes ---------------------------------------------------------------------------

/// Idempotently record a citizen's vote on a comment. The `ON CONFLICT` upsert keys on the
/// `(comment_id, citizen_id)` UNIQUE constraint: a re-vote updates the weight in place and
/// `RETURNING` yields the REAL stored row — including the original `id` and `created_at`,
/// never a phantom pre-generated id. The caller supplies the id used only on first insert.
///
/// # Errors
/// [`Error::Db`] on a storage failure (e.g. unknown `comment_id`/`citizen_id`, or a weight
/// outside the `{-1, +1}` CHECK).
pub async fn upsert_vote<'e, E>(
    executor: E,
    id: Uuid,
    comment_id: Uuid,
    citizen_id: Uuid,
    weight: i16,
    created_at: DateTime<Utc>,
) -> Result<CommentVote, Error>
where
    E: PgExecutor<'e>,
{
    let row = sqlx::query_as!(
        VoteRow,
        r#"
        INSERT INTO comment_vote (id, comment_id, citizen_id, weight, created_at)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (comment_id, citizen_id)
        DO UPDATE SET weight = EXCLUDED.weight
        RETURNING id, comment_id, citizen_id, weight, created_at
        "#,
        id,
        comment_id,
        citizen_id,
        weight,
        created_at,
    )
    .fetch_one(executor)
    .await?;
    Ok(row.into())
}

/// Sum the up/down weights of a comment (its net score). Returns `0` for a comment with no
/// votes. Used to expose an aggregate without leaking per-citizen linkage.
///
/// # Errors
/// [`Error::Db`] on a storage failure.
pub async fn comment_score(db: &Db, comment_id: Uuid) -> Result<i64, Error> {
    let row = sqlx::query!(
        r#"
        SELECT COALESCE(SUM(weight), 0) AS "score!"
        FROM comment_vote
        WHERE comment_id = $1
        "#,
        comment_id,
    )
    .fetch_one(db)
    .await?;
    Ok(row.score)
}

fn split_cursor(cursor: Option<Cursor>) -> (Option<DateTime<Utc>>, Option<Uuid>) {
    match cursor {
        Some(c) => (Some(c.at), Some(c.id)),
        None => (None, None),
    }
}

//! Every database statement for debates, as `sqlx` compile-time-checked queries (PLAN.md
//! principle 3 — no ORM, no `SELECT *`, keyset pagination on unbounded reads).
//!
//! Row shapes are returned to the service, which maps `sqlx` failures onto the canonical
//! [`dsoc_core::Error`]. Inserts use `RETURNING` so the caller always gets the real stored
//! row (never a phantom pre-generated value). Lists keyset-paginate over the ascending
//! UUIDv7 `id` (time-ordered), scoped by their parent.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// A persisted debate row (`debate`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebateRow {
    /// Debate id.
    pub id: Uuid,
    /// Owning organization.
    pub org_id: Uuid,
    /// Title — the motion under debate.
    pub title: String,
    /// Framing — the neutral context.
    pub framing: String,
    /// Optional UF territorial scope (`None` = nacional).
    pub uf: Option<String>,
    /// Creation time (from the injected clock).
    pub created_at: DateTime<Utc>,
}

/// A persisted contribution row (`debate_contribution`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContributionRow {
    /// Contribution id.
    pub id: Uuid,
    /// The debate this contribution belongs to.
    pub debate_id: Uuid,
    /// Author (a `citizen`).
    pub author_id: Uuid,
    /// Stored stance token (`pro` | `con` | `neutral`).
    pub stance: String,
    /// The contribution text.
    pub body: String,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

/// Insert a debate and return the stored row.
///
/// # Errors
/// Propagates the underlying `sqlx::Error` (e.g. unknown `org_id`, or a CHECK violation).
pub async fn insert_debate(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    org_id: Uuid,
    title: &str,
    framing: &str,
    uf: Option<&str>,
    created_at: DateTime<Utc>,
) -> Result<DebateRow, sqlx::Error> {
    let row = sqlx::query!(
        r#"INSERT INTO debate (id, org_id, title, framing, uf, created_at)
           VALUES ($1, $2, $3, $4, $5, $6)
           RETURNING id, org_id, title, framing, uf, created_at"#,
        id,
        org_id,
        title,
        framing,
        uf,
        created_at,
    )
    .fetch_one(executor)
    .await?;
    Ok(DebateRow {
        id: row.id,
        org_id: row.org_id,
        title: row.title,
        framing: row.framing,
        uf: row.uf,
        created_at: row.created_at,
    })
}

/// Fetch a single debate by id.
///
/// # Errors
/// Propagates the underlying `sqlx::Error` (including `RowNotFound` when absent).
pub async fn get_debate(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
) -> Result<DebateRow, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT id, org_id, title, framing, uf, created_at
           FROM debate
           WHERE id = $1"#,
        id,
    )
    .fetch_one(executor)
    .await?;
    Ok(DebateRow {
        id: row.id,
        org_id: row.org_id,
        title: row.title,
        framing: row.framing,
        uf: row.uf,
        created_at: row.created_at,
    })
}

/// List an org's debates with keyset pagination over ascending `id` (UUIDv7 = time-ordered).
/// `after` is the last id seen; `None` starts from the beginning.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn list_debates(
    executor: impl sqlx::PgExecutor<'_>,
    org_id: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<DebateRow>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT id, org_id, title, framing, uf, created_at
           FROM debate
           WHERE org_id = $1 AND ($2::uuid IS NULL OR id > $2)
           ORDER BY id
           LIMIT $3"#,
        org_id,
        after,
        limit,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| DebateRow {
            id: row.id,
            org_id: row.org_id,
            title: row.title,
            framing: row.framing,
            uf: row.uf,
            created_at: row.created_at,
        })
        .collect())
}

/// Count the debates owned by an org (for pagination metadata).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn count_debates(
    executor: impl sqlx::PgExecutor<'_>,
    org_id: Uuid,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT count(*) AS "count!: i64" FROM debate WHERE org_id = $1"#,
        org_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(row.count)
}

/// Insert a contribution and return the stored row.
///
/// # Errors
/// Propagates the underlying `sqlx::Error` (e.g. unknown `debate_id`/`author_id`, or a
/// stance/body CHECK violation).
pub async fn insert_contribution(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    debate_id: Uuid,
    author_id: Uuid,
    stance: &str,
    body: &str,
    created_at: DateTime<Utc>,
) -> Result<ContributionRow, sqlx::Error> {
    let row = sqlx::query!(
        r#"INSERT INTO debate_contribution
               (id, debate_id, author_id, stance, body, created_at)
           VALUES ($1, $2, $3, $4, $5, $6)
           RETURNING id, debate_id, author_id, stance, body, created_at"#,
        id,
        debate_id,
        author_id,
        stance,
        body,
        created_at,
    )
    .fetch_one(executor)
    .await?;
    Ok(ContributionRow {
        id: row.id,
        debate_id: row.debate_id,
        author_id: row.author_id,
        stance: row.stance,
        body: row.body,
        created_at: row.created_at,
    })
}

/// List a debate's contributions with keyset pagination over ascending `id` (oldest-first).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn list_contributions(
    executor: impl sqlx::PgExecutor<'_>,
    debate_id: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<ContributionRow>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT id, debate_id, author_id, stance, body, created_at
           FROM debate_contribution
           WHERE debate_id = $1 AND ($2::uuid IS NULL OR id > $2)
           ORDER BY id
           LIMIT $3"#,
        debate_id,
        after,
        limit,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| ContributionRow {
            id: row.id,
            debate_id: row.debate_id,
            author_id: row.author_id,
            stance: row.stance,
            body: row.body,
            created_at: row.created_at,
        })
        .collect())
}

/// Count the contributions on a debate (for pagination metadata).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn count_contributions(
    executor: impl sqlx::PgExecutor<'_>,
    debate_id: Uuid,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT count(*) AS "count!: i64" FROM debate_contribution WHERE debate_id = $1"#,
        debate_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(row.count)
}

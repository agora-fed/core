//! All persistence for `consequence`. Every statement is an explicit, compile-time-checked
//! `sqlx::query!` (PLAN.md principle 3): no ORM, no query builder, no `SELECT *`, keyset pagination.
//!
//! Two invariants are enforced at the SQL layer:
//! - **Idempotent SLA start.** `insert_sla_idempotent` upserts on `(proposal_id, cluster_id)` with
//!   `ON CONFLICT DO NOTHING RETURNING`, so a duplicate `proposals.threshold.crossed` (at-least-once
//!   delivery) cannot start a second clock; on conflict the caller re-selects the REAL stored row.
//! - **Guarded transitions.** `update_sla_status_guarded` carries the expected prior status in its
//!   `WHERE`, closing the TOCTOU window; the row is also taken `FOR UPDATE` first. A write-once
//!   `consequence_outcome` row makes the first terminal decision permanent.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// A consequence SLA row (full projection; no `SELECT *`).
#[derive(Debug, Clone)]
pub struct SlaRow {
    /// SLA id.
    pub id: Uuid,
    /// Owning organization.
    pub org_id: Uuid,
    /// The targeted official's mandate.
    pub mandate_id: Uuid,
    /// The consensus cluster the demand belongs to.
    pub cluster_id: Uuid,
    /// The proposal that crossed the threshold.
    pub proposal_id: Uuid,
    /// When the clock started.
    pub started_at: DateTime<Utc>,
    /// The response deadline.
    pub due_at: DateTime<Utc>,
    /// Canonical status text (`pending`/`answered`/`acted`/`ignored`).
    pub status: String,
    /// Row creation time.
    pub created_at: DateTime<Utc>,
}

/// A pending SLA selected by the expiry sweep.
#[derive(Debug, Clone, Copy)]
pub struct ExpiredCandidate {
    /// SLA id.
    pub id: Uuid,
    /// Owning organization (needed to stamp the emitted event envelope).
    pub org_id: Uuid,
    /// The targeted official's mandate (carried in `consequence.sla.expired`).
    pub mandate_id: Uuid,
    /// The deadline that has passed.
    pub due_at: DateTime<Utc>,
}

/// Idempotently insert an SLA clock. On a duplicate `(proposal_id, cluster_id, mandate_id)` — desde
/// since 0538 the clock is PER OFFICE — the insert is a no-op and this returns `None`; the caller
/// re-selects the real stored row via [`get_sla_by_proposal_cluster`]. When `Some`, the row was
/// freshly inserted (emit `started`).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
#[allow(clippy::too_many_arguments)]
pub async fn insert_sla_idempotent(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    org_id: Uuid,
    mandate_id: Uuid,
    cluster_id: Uuid,
    proposal_id: Uuid,
    started_at: DateTime<Utc>,
    due_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
) -> Result<Option<SlaRow>, sqlx::Error> {
    let row = sqlx::query!(
        r#"INSERT INTO consequence_sla
               (id, org_id, mandate_id, cluster_id, proposal_id, started_at, due_at, status, created_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, 'pending', $8)
           ON CONFLICT (proposal_id, cluster_id, mandate_id) DO NOTHING
           RETURNING id, org_id, mandate_id, cluster_id, proposal_id, started_at, due_at, status, created_at"#,
        id,
        org_id,
        mandate_id,
        cluster_id,
        proposal_id,
        started_at,
        due_at,
        created_at,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row.map(|r| SlaRow {
        id: r.id,
        org_id: r.org_id,
        mandate_id: r.mandate_id,
        cluster_id: r.cluster_id,
        proposal_id: r.proposal_id,
        started_at: r.started_at,
        due_at: r.due_at,
        status: r.status,
        created_at: r.created_at,
    }))
}

/// Re-select the SLA stored for a `(proposal_id, cluster_id, mandate_id)` triple after an
/// idempotent-insert conflict, so the caller returns the REAL stored id rather than a phantom one.
/// Since 0538 the clock is PER OFFICE — the same demand opens one SLA per recipient.
///
/// # Errors
/// Propagates the underlying `sqlx::Error` (including `RowNotFound`).
pub async fn get_sla_by_proposal_cluster(
    executor: impl sqlx::PgExecutor<'_>,
    proposal_id: Uuid,
    cluster_id: Uuid,
    mandate_id: Uuid,
) -> Result<SlaRow, sqlx::Error> {
    let r = sqlx::query!(
        r#"SELECT id, org_id, mandate_id, cluster_id, proposal_id, started_at, due_at, status, created_at
           FROM consequence_sla
           WHERE proposal_id = $1 AND cluster_id = $2 AND mandate_id = $3"#,
        proposal_id,
        cluster_id,
        mandate_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(SlaRow {
        id: r.id,
        org_id: r.org_id,
        mandate_id: r.mandate_id,
        cluster_id: r.cluster_id,
        proposal_id: r.proposal_id,
        started_at: r.started_at,
        due_at: r.due_at,
        status: r.status,
        created_at: r.created_at,
    })
}

/// Fetch a single SLA by id.
///
/// # Errors
/// Propagates the underlying `sqlx::Error` (including `RowNotFound`).
pub async fn get_sla(executor: impl sqlx::PgExecutor<'_>, id: Uuid) -> Result<SlaRow, sqlx::Error> {
    let r = sqlx::query!(
        r#"SELECT id, org_id, mandate_id, cluster_id, proposal_id, started_at, due_at, status, created_at
           FROM consequence_sla
           WHERE id = $1"#,
        id,
    )
    .fetch_one(executor)
    .await?;
    Ok(SlaRow {
        id: r.id,
        org_id: r.org_id,
        mandate_id: r.mandate_id,
        cluster_id: r.cluster_id,
        proposal_id: r.proposal_id,
        started_at: r.started_at,
        due_at: r.due_at,
        status: r.status,
        created_at: r.created_at,
    })
}

/// Lock a single SLA row `FOR UPDATE` (inside the caller's transaction) so a state transition is
/// evaluated and applied without a TOCTOU race against a concurrent sweep or response.
///
/// # Errors
/// Propagates the underlying `sqlx::Error` (including `RowNotFound`).
pub async fn lock_sla(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
) -> Result<SlaRow, sqlx::Error> {
    let r = sqlx::query!(
        r#"SELECT id, org_id, mandate_id, cluster_id, proposal_id, started_at, due_at, status, created_at
           FROM consequence_sla
           WHERE id = $1
           FOR UPDATE"#,
        id,
    )
    .fetch_one(executor)
    .await?;
    Ok(SlaRow {
        id: r.id,
        org_id: r.org_id,
        mandate_id: r.mandate_id,
        cluster_id: r.cluster_id,
        proposal_id: r.proposal_id,
        started_at: r.started_at,
        due_at: r.due_at,
        status: r.status,
        created_at: r.created_at,
    })
}

/// Apply a guarded status transition: move the SLA to `next` only if it is currently `expected_prior`.
/// Returns `true` when the row moved (one row affected), `false` when the guard did not match (already
/// transitioned by a concurrent actor — treat as the already-in-target case per the wiring rules).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn update_sla_status_guarded(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    expected_prior: &str,
    next: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        r#"UPDATE consequence_sla
           SET status = $3
           WHERE id = $1 AND status = $2"#,
        id,
        expected_prior,
        next,
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Append an official response to the audit history. Returns the stored response id.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
#[allow(clippy::too_many_arguments)]
pub async fn insert_response(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    sla_id: Uuid,
    mandate_id: Uuid,
    body: &str,
    committed: bool,
    responded_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
) -> Result<Uuid, sqlx::Error> {
    let r = sqlx::query!(
        r#"INSERT INTO consequence_response
               (id, sla_id, mandate_id, body, committed, responded_at, created_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7)
           RETURNING id"#,
        id,
        sla_id,
        mandate_id,
        body,
        committed,
        responded_at,
        created_at,
    )
    .fetch_one(executor)
    .await?;
    Ok(r.id)
}

/// Write the terminal, WRITE-ONCE outcome for an SLA. On a duplicate `sla_id` the insert is a no-op
/// and this returns `false`: the first terminal decision (especially recorded silence) is permanent.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn insert_outcome_writeonce(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    sla_id: Uuid,
    outcome: &str,
    decided_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
) -> Result<bool, sqlx::Error> {
    let row = sqlx::query!(
        r#"INSERT INTO consequence_outcome (id, sla_id, outcome, decided_at, created_at)
           VALUES ($1, $2, $3, $4, $5)
           ON CONFLICT (sla_id) DO NOTHING
           RETURNING id"#,
        id,
        sla_id,
        outcome,
        decided_at,
        created_at,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row.is_some())
}

/// Fetch the recorded outcome text for an SLA, if any (read surface for the public scorecard / tests).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn get_outcome(
    executor: impl sqlx::PgExecutor<'_>,
    sla_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT outcome FROM consequence_outcome WHERE sla_id = $1"#,
        sla_id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row.map(|r| r.outcome))
}

/// Select an org's pending SLAs whose deadline has passed, locking them `FOR UPDATE SKIP LOCKED` so
/// concurrent sweepers partition the work without blocking. Scoped to `org_id`: a per-org sweep must
/// never touch another tenant's clocks (the sweep endpoint is authorized per org). Bounded by `limit`
/// (the sweep is repeatable).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
/// Distinct orgs that currently have at least one `pending` SLA already past its `due_at`. Drives the
/// background sweep so it only touches tenants with work, scoped per-org for the existing batch sweep.
pub async fn select_due_org_ids(
    executor: impl sqlx::PgExecutor<'_>,
    now: DateTime<Utc>,
) -> Result<Vec<Uuid>, sqlx::Error> {
    // Runtime-bound (not the `query!` macro) so this internal scheduler enumeration needs no `.sqlx`
    // cache entry — the offline build never depends on it. The SQL is a fixed literal with one typed
    // bind; there is no injection surface.
    sqlx::query_scalar::<_, Uuid>(
        "SELECT DISTINCT org_id FROM consequence_sla WHERE status = 'pending' AND due_at <= $1",
    )
    .bind(now)
    .fetch_all(executor)
    .await
}

pub async fn select_expired_pending(
    executor: impl sqlx::PgExecutor<'_>,
    org_id: Uuid,
    now: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<ExpiredCandidate>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT id, org_id, mandate_id, due_at
           FROM consequence_sla
           WHERE org_id = $1 AND status = 'pending' AND due_at <= $2
           ORDER BY id
           LIMIT $3
           FOR UPDATE SKIP LOCKED"#,
        org_id,
        now,
        limit,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| ExpiredCandidate {
            id: r.id,
            org_id: r.org_id,
            mandate_id: r.mandate_id,
            due_at: r.due_at,
        })
        .collect())
}

/// List an org's SLAs with keyset pagination over the ascending `id` (UUIDv7 = time-ordered).
/// `after` is the last id seen; `None` starts from the beginning.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn list_slas(
    executor: impl sqlx::PgExecutor<'_>,
    org_id: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<SlaRow>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT id, org_id, mandate_id, cluster_id, proposal_id, started_at, due_at, status, created_at
           FROM consequence_sla
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
        .map(|r| SlaRow {
            id: r.id,
            org_id: r.org_id,
            mandate_id: r.mandate_id,
            cluster_id: r.cluster_id,
            proposal_id: r.proposal_id,
            started_at: r.started_at,
            due_at: r.due_at,
            status: r.status,
            created_at: r.created_at,
        })
        .collect())
}

/// Count the SLAs owned by an org (for pagination metadata).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn count_slas(
    executor: impl sqlx::PgExecutor<'_>,
    org_id: Uuid,
) -> Result<i64, sqlx::Error> {
    let r = sqlx::query!(
        r#"SELECT count(*) AS "count!: i64"
           FROM consequence_sla
           WHERE org_id = $1"#,
        org_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(r.count)
}

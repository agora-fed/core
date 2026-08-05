//! All persistence for `proposals`. Every statement is an explicit, compile-time-checked
//! `sqlx::query!` (PLAN.md principle 3): no ORM, no query builder, no `SELECT *`, keyset pagination.
//!
//! State-transition UPDATEs are guarded by the expected prior state (`WHERE … IS NULL`) so they are
//! idempotent under at-least-once delivery and race-free under the row lock the service takes with
//! [`lock_proposal`]; the caller treats `rows_affected == 0` as the already-in-target case.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// A proposal row for reads and locked updates.
#[derive(Debug, Clone)]
pub struct ProposalRow {
    /// Proposal id.
    pub id: Uuid,
    /// Owning organization.
    pub org_id: Uuid,
    /// Mandate this proposal is directed at.
    pub mandate_id: Uuid,
    /// Consensus cluster this proposal was merged into, if any.
    pub cluster_id: Option<Uuid>,
    /// Title (Portuguese civic content).
    pub title: String,
    /// Body / description.
    pub body: String,
    /// Lifecycle status text (`draft` / `published` / `clustered`).
    pub status: String,
    /// Aggregate support tally (maintained from the event stream).
    pub support_count: i64,
    /// Support count at which the consequence loop fires.
    pub threshold: i32,
    /// When support crossed the threshold while clustered (idempotency guard), if it has.
    pub threshold_crossed_at: Option<DateTime<Utc>>,
    /// When the proposal was published, if it has been.
    pub published_at: Option<DateTime<Utc>>,
    /// Citizen who proposed (`None` for legacy / platform-seeded rows).
    pub author_citizen_id: Option<Uuid>,
    /// Author user-chosen handle (JOIN from citizen). `None` if no author OR author hasn't set
    /// a handle yet.
    pub author_handle: Option<String>,
    /// Author avatar object key (JOIN from citizen). Composed into a URL at the http layer.
    pub author_avatar_object_key: Option<String>,
    /// Urgency level (`comum` | `urgente`), migration 0302. Voting `urgente` requires
    /// `titulo_status ∈ (validated,verified)` — gated in the votes crate (0.25.0-fediverse).
    pub urgencia: String,
    /// Timestamp of the confirmation e-mail to the author (migration 0303).
    pub notified_author_at: Option<DateTime<Utc>>,
    /// Timestamp of the e-mail delivered to the cabinet (migration 0303).
    pub notified_mandate_at: Option<DateTime<Utc>>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

/// One recipient of the proposal (0537) with the mandate's display data (JOIN).
#[derive(Debug, Clone)]
pub struct TargetRow {
    /// The recipient mandate.
    pub mandate_id: Uuid,
    /// Public name of the mandate.
    pub display_name: String,
    /// Cargo (ex.: `deputado_federal`).
    pub office: String,
    /// Esfera federativa (`federal` | `estadual` | `municipal`).
    pub sphere: String,
    /// Delivery receipt to the office (the e-mail left the relay), when it has.
    pub notified_at: Option<DateTime<Utc>>,
}

/// Sphere check of the recipients: how many of the ids exist and how many distinct
/// spheres they span. The service requires `found == requested` and `spheres == 1`.
#[derive(Debug, Clone, Copy)]
pub struct SphereCheck {
    /// How many of the requested mandates exist.
    pub found: i64,
    /// Quantas esferas distintas o conjunto cobre.
    pub spheres: i64,
}

/// An append-only revision row.
#[derive(Debug, Clone)]
pub struct RevisionRow {
    /// Revision id.
    pub id: Uuid,
    /// Proposal this revision belongs to.
    pub proposal_id: Uuid,
    /// Title at this revision.
    pub title: String,
    /// Body at this revision.
    pub body: String,
    /// Citizen who authored this revision.
    pub edited_by: Uuid,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

/// Insert a new proposal (status defaults to `draft`, support to 0) and return the stored row.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
#[allow(clippy::too_many_arguments)]
pub async fn insert_proposal(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    org_id: Uuid,
    mandate_id: Uuid,
    title: &str,
    body: &str,
    threshold: i32,
    created_at: DateTime<Utc>,
    author_citizen_id: Option<Uuid>,
) -> Result<ProposalRow, sqlx::Error> {
    let row = sqlx::query!(
        r#"INSERT INTO proposal (id, org_id, mandate_id, title, body, threshold, created_at, author_citizen_id)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
           RETURNING id, org_id, mandate_id, cluster_id, title, body, status,
                     support_count, threshold, threshold_crossed_at, published_at,
                     author_citizen_id, urgencia, notified_author_at, notified_mandate_at, created_at"#,
        id,
        org_id,
        mandate_id,
        title,
        body,
        threshold,
        created_at,
        author_citizen_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(ProposalRow {
        id: row.id,
        org_id: row.org_id,
        mandate_id: row.mandate_id,
        cluster_id: row.cluster_id,
        title: row.title,
        body: row.body,
        status: row.status,
        support_count: row.support_count,
        threshold: row.threshold,
        threshold_crossed_at: row.threshold_crossed_at,
        published_at: row.published_at,
        author_citizen_id: row.author_citizen_id,
        // INSERT path doesn't surface handle/avatar — the freshly-inserted DTO doesn't need them
        // for the immediate response; the read path joins citizen and surfaces them.
        author_handle: None,
        author_avatar_object_key: None,
        urgencia: row.urgencia,
        notified_author_at: row.notified_author_at,
        notified_mandate_at: row.notified_mandate_at,
        created_at: row.created_at,
    })
}

/// Counts the existing mandates and the distinct spheres of a set of ids (0537).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn check_target_spheres(
    executor: impl sqlx::PgExecutor<'_>,
    mandate_ids: &[Uuid],
) -> Result<SphereCheck, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT count(*) AS "found!: i64", count(DISTINCT sphere) AS "spheres!: i64"
           FROM mandate WHERE id = ANY($1)"#,
        mandate_ids,
    )
    .fetch_one(executor)
    .await?;
    Ok(SphereCheck {
        found: row.found,
        spheres: row.spheres,
    })
}

/// Insert one recipient of the proposal (0537). Idempotency under the composite PK is
/// unnecessary here: the service only inserts on create, inside the same transaction.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn insert_target(
    executor: impl sqlx::PgExecutor<'_>,
    proposal_id: Uuid,
    mandate_id: Uuid,
    created_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"INSERT INTO proposal_target (proposal_id, mandate_id, created_at)
           VALUES ($1, $2, $3)"#,
        proposal_id,
        mandate_id,
        created_at,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// List a proposal's recipients with the mandate's display data,
/// primary first (insertion order via created_at, tie-broken by mandate_id).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn list_targets(
    executor: impl sqlx::PgExecutor<'_>,
    proposal_id: Uuid,
) -> Result<Vec<TargetRow>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT pt.mandate_id, pt.notified_at,
                  m.display_name, m.office, m.sphere
           FROM proposal_target pt
           JOIN mandate m ON m.id = pt.mandate_id
           WHERE pt.proposal_id = $1
           ORDER BY pt.created_at, pt.mandate_id"#,
        proposal_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| TargetRow {
            mandate_id: row.mandate_id,
            display_name: row.display_name,
            office: row.office,
            sphere: row.sphere,
            notified_at: row.notified_at,
        })
        .collect())
}

/// Only the recipient mandate ids (primary first) — the light version of
/// [`list_targets`] for use under the crossing row lock (no display JOIN).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn target_mandate_ids(
    executor: impl sqlx::PgExecutor<'_>,
    proposal_id: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query_scalar!(
        r#"SELECT mandate_id FROM proposal_target
           WHERE proposal_id = $1
           ORDER BY created_at, mandate_id"#,
        proposal_id,
    )
    .fetch_all(executor)
    .await
}

/// Fetch a single proposal by id.
///
/// # Errors
/// Propagates the underlying `sqlx::Error` (including `RowNotFound`).
pub async fn get_proposal(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
) -> Result<ProposalRow, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT p.id, p.org_id, p.mandate_id, p.cluster_id, p.title, p.body, p.status,
                  p.support_count, p.threshold, p.threshold_crossed_at, p.published_at,
                  p.author_citizen_id, p.urgencia, p.notified_author_at, p.notified_mandate_at, p.created_at,
                  c.handle              AS "author_handle?",
                  c.avatar_object_key   AS "author_avatar_object_key?"
           FROM proposal p
           LEFT JOIN citizen c ON c.id = p.author_citizen_id
           WHERE p.id = $1"#,
        id,
    )
    .fetch_one(executor)
    .await?;
    Ok(ProposalRow {
        id: row.id,
        org_id: row.org_id,
        mandate_id: row.mandate_id,
        cluster_id: row.cluster_id,
        title: row.title,
        body: row.body,
        status: row.status,
        support_count: row.support_count,
        threshold: row.threshold,
        threshold_crossed_at: row.threshold_crossed_at,
        published_at: row.published_at,
        author_citizen_id: row.author_citizen_id,
        author_handle: row.author_handle,
        author_avatar_object_key: row.author_avatar_object_key,
        urgencia: row.urgencia,
        notified_author_at: row.notified_author_at,
        notified_mandate_at: row.notified_mandate_at,
        created_at: row.created_at,
    })
}

/// Lock a proposal row `FOR UPDATE` and return it, or `None` if it does not exist. Closes the
/// TOCTOU window for state transitions: the caller reads, decides, and writes while holding the lock.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn lock_proposal(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
) -> Result<Option<ProposalRow>, sqlx::Error> {
    // Internal locked read — author handle/avatar are presentation data, never consulted under
    // the lock, so we skip the JOIN to keep the lock tight.
    let row = sqlx::query!(
        r#"SELECT id, org_id, mandate_id, cluster_id, title, body, status,
                  support_count, threshold, threshold_crossed_at, published_at,
                  author_citizen_id, urgencia, notified_author_at, notified_mandate_at, created_at
           FROM proposal
           WHERE id = $1
           FOR UPDATE"#,
        id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row.map(|row| ProposalRow {
        id: row.id,
        org_id: row.org_id,
        mandate_id: row.mandate_id,
        cluster_id: row.cluster_id,
        title: row.title,
        body: row.body,
        status: row.status,
        support_count: row.support_count,
        threshold: row.threshold,
        threshold_crossed_at: row.threshold_crossed_at,
        published_at: row.published_at,
        author_citizen_id: row.author_citizen_id,
        author_handle: None,
        author_avatar_object_key: None,
        urgencia: row.urgencia,
        notified_author_at: row.notified_author_at,
        notified_mandate_at: row.notified_mandate_at,
        created_at: row.created_at,
    }))
}

/// List an org's proposals with keyset pagination over the ascending `id` (UUIDv7 = time-ordered).
/// `after` is the last id seen; `None` starts from the beginning.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn list_proposals(
    executor: impl sqlx::PgExecutor<'_>,
    org_id: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<ProposalRow>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT p.id, p.org_id, p.mandate_id, p.cluster_id, p.title, p.body, p.status,
                  p.support_count, p.threshold, p.threshold_crossed_at, p.published_at,
                  p.author_citizen_id, p.urgencia, p.notified_author_at, p.notified_mandate_at, p.created_at,
                  c.handle              AS "author_handle?",
                  c.avatar_object_key   AS "author_avatar_object_key?"
           FROM proposal p
           LEFT JOIN citizen c ON c.id = p.author_citizen_id
           WHERE p.org_id = $1 AND p.hidden_at IS NULL AND ($2::uuid IS NULL OR p.id > $2)
           ORDER BY p.id
           LIMIT $3"#,
        org_id,
        after,
        limit,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| ProposalRow {
            id: row.id,
            org_id: row.org_id,
            mandate_id: row.mandate_id,
            cluster_id: row.cluster_id,
            title: row.title,
            body: row.body,
            status: row.status,
            support_count: row.support_count,
            threshold: row.threshold,
            threshold_crossed_at: row.threshold_crossed_at,
            published_at: row.published_at,
            author_citizen_id: row.author_citizen_id,
            author_handle: row.author_handle,
            author_avatar_object_key: row.author_avatar_object_key,
            urgencia: row.urgencia,
            notified_author_at: row.notified_author_at,
            notified_mandate_at: row.notified_mandate_at,
            created_at: row.created_at,
        })
        .collect())
}

/// Count the proposals owned by an org (for pagination metadata).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn count_proposals(
    executor: impl sqlx::PgExecutor<'_>,
    org_id: Uuid,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT count(*) AS "count!: i64" FROM proposal WHERE org_id = $1"#,
        org_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(row.count)
}

/// Set a proposal's aggregate support count (already folded monotonically by the caller). The caller
/// holds the row lock from [`lock_proposal`].
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn set_support_count(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    support_count: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"UPDATE proposal SET support_count = $2 WHERE id = $1"#,
        id,
        support_count,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// Link a proposal to a consensus cluster and mark it `clustered`, **guarded** by `cluster_id IS
/// NULL` so a duplicate `consensus.proposal.merged` delivery is a no-op (idempotent). Returns the
/// number of rows affected: `1` on the first link, `0` if already linked.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn link_cluster(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    cluster_id: Uuid,
) -> Result<u64, sqlx::Error> {
    let res = sqlx::query!(
        r#"UPDATE proposal
           SET cluster_id = $2, status = 'clustered'
           WHERE id = $1 AND cluster_id IS NULL"#,
        id,
        cluster_id,
    )
    .execute(executor)
    .await?;
    Ok(res.rows_affected())
}

/// Publish a proposal, **guarded** by `published_at IS NULL` so a duplicate `moderation.cleared`
/// delivery is a no-op (idempotent). Returns rows affected: `1` on first publish, `0` if already
/// published. Status is set to `published` only when it is still `draft`, so a proposal already
/// `clustered` keeps its clustered status while still recording `published_at`.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn publish(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    published_at: DateTime<Utc>,
) -> Result<u64, sqlx::Error> {
    let res = sqlx::query!(
        r#"UPDATE proposal
           SET published_at = $2,
               status = CASE WHEN status = 'draft' THEN 'published' ELSE status END
           WHERE id = $1 AND published_at IS NULL"#,
        id,
        published_at,
    )
    .execute(executor)
    .await?;
    Ok(res.rows_affected())
}

/// Fire the threshold crossing exactly once, **guarded** by `threshold_crossed_at IS NULL`. Returns
/// rows affected: `1` the first time the crossing is recorded, `0` on any duplicate signal. The
/// caller only invokes this after the pure policy ([`crate::domain::crossing_fires`]) holds.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn mark_threshold_crossed(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    crossed_at: DateTime<Utc>,
) -> Result<u64, sqlx::Error> {
    let res = sqlx::query!(
        r#"UPDATE proposal
           SET threshold_crossed_at = $2
           WHERE id = $1 AND threshold_crossed_at IS NULL"#,
        id,
        crossed_at,
    )
    .execute(executor)
    .await?;
    Ok(res.rows_affected())
}

/// Append a revision (history is never updated/deleted) and return the stored row. The caller holds
/// the row lock from [`lock_proposal`].
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn insert_revision(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    proposal_id: Uuid,
    title: &str,
    body: &str,
    edited_by: Uuid,
    created_at: DateTime<Utc>,
) -> Result<RevisionRow, sqlx::Error> {
    let row = sqlx::query!(
        r#"INSERT INTO proposal_revision (id, proposal_id, title, body, edited_by, created_at)
           VALUES ($1, $2, $3, $4, $5, $6)
           RETURNING id, proposal_id, title, body, edited_by, created_at"#,
        id,
        proposal_id,
        title,
        body,
        edited_by,
        created_at,
    )
    .fetch_one(executor)
    .await?;
    Ok(RevisionRow {
        id: row.id,
        proposal_id: row.proposal_id,
        title: row.title,
        body: row.body,
        edited_by: row.edited_by,
        created_at: row.created_at,
    })
}

/// Mirror the latest revision's title/body onto the proposal so reads see the latest content. The
/// caller holds the row lock.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn update_content(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    title: &str,
    body: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"UPDATE proposal SET title = $2, body = $3 WHERE id = $1"#,
        id,
        title,
        body,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// List a proposal's revisions oldest-first with keyset pagination over ascending `id`.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn list_revisions(
    executor: impl sqlx::PgExecutor<'_>,
    proposal_id: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<RevisionRow>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT id, proposal_id, title, body, edited_by, created_at
           FROM proposal_revision
           WHERE proposal_id = $1 AND ($2::uuid IS NULL OR id > $2)
           ORDER BY id
           LIMIT $3"#,
        proposal_id,
        after,
        limit,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| RevisionRow {
            id: row.id,
            proposal_id: row.proposal_id,
            title: row.title,
            body: row.body,
            edited_by: row.edited_by,
            created_at: row.created_at,
        })
        .collect())
}

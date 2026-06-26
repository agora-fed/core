//! Every database statement for budgets, as `sqlx` compile-time-checked queries (PLAN.md
//! principle 3 — no ORM, no `SELECT *`, keyset pagination on unbounded reads).
//!
//! Row shapes are returned to the service, which maps `sqlx` failures onto the canonical
//! [`dsoc_core::Error`]. Inserts use `RETURNING` so the caller always gets the real stored
//! row (never a phantom pre-generated value). Lists keyset-paginate over the ascending
//! UUIDv7 `id` (time-ordered), scoped by their parent.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// A persisted budget row (`budget`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetRow {
    /// Budget id.
    pub id: Uuid,
    /// Owning organization.
    pub org_id: Uuid,
    /// Title of the budgeting round.
    pub title: String,
    /// Spending ceiling, in cents.
    pub ceiling_cents: i64,
    /// Creation time (from the injected clock).
    pub created_at: DateTime<Utc>,
}

/// A persisted project row (`budget_project`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRow {
    /// Project id.
    pub id: Uuid,
    /// The budget this project belongs to.
    pub budget_id: Uuid,
    /// Title of the project.
    pub title: String,
    /// Cost, in cents.
    pub cost_cents: i64,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

/// A persisted order row (`budget_order`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderRow {
    /// Order id.
    pub id: Uuid,
    /// The budget this order allocates within.
    pub budget_id: Uuid,
    /// The citizen who placed the order.
    pub citizen_id: Uuid,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

/// Insert a budget and return the stored row.
///
/// # Errors
/// Propagates the underlying `sqlx::Error` (e.g. unknown `org_id`, or a CHECK violation).
pub async fn insert_budget(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    org_id: Uuid,
    title: &str,
    ceiling_cents: i64,
    created_at: DateTime<Utc>,
) -> Result<BudgetRow, sqlx::Error> {
    let row = sqlx::query!(
        r#"INSERT INTO budget (id, org_id, title, ceiling_cents, created_at)
           VALUES ($1, $2, $3, $4, $5)
           RETURNING id, org_id, title, ceiling_cents, created_at"#,
        id,
        org_id,
        title,
        ceiling_cents,
        created_at,
    )
    .fetch_one(executor)
    .await?;
    Ok(BudgetRow {
        id: row.id,
        org_id: row.org_id,
        title: row.title,
        ceiling_cents: row.ceiling_cents,
        created_at: row.created_at,
    })
}

/// Fetch a single budget by id.
///
/// # Errors
/// Propagates the underlying `sqlx::Error` (including `RowNotFound` when absent).
pub async fn get_budget(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
) -> Result<BudgetRow, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT id, org_id, title, ceiling_cents, created_at
           FROM budget
           WHERE id = $1"#,
        id,
    )
    .fetch_one(executor)
    .await?;
    Ok(BudgetRow {
        id: row.id,
        org_id: row.org_id,
        title: row.title,
        ceiling_cents: row.ceiling_cents,
        created_at: row.created_at,
    })
}

/// List an org's budgets with keyset pagination over ascending `id` (UUIDv7 = time-ordered).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn list_budgets(
    executor: impl sqlx::PgExecutor<'_>,
    org_id: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<BudgetRow>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT id, org_id, title, ceiling_cents, created_at
           FROM budget
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
        .map(|row| BudgetRow {
            id: row.id,
            org_id: row.org_id,
            title: row.title,
            ceiling_cents: row.ceiling_cents,
            created_at: row.created_at,
        })
        .collect())
}

/// Count the budgets owned by an org (for pagination metadata).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn count_budgets(
    executor: impl sqlx::PgExecutor<'_>,
    org_id: Uuid,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT count(*) AS "count!: i64" FROM budget WHERE org_id = $1"#,
        org_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(row.count)
}

/// Insert a project under a budget and return the stored row.
///
/// # Errors
/// Propagates the underlying `sqlx::Error` (e.g. unknown `budget_id`, or a CHECK violation).
pub async fn insert_project(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    budget_id: Uuid,
    title: &str,
    cost_cents: i64,
    created_at: DateTime<Utc>,
) -> Result<ProjectRow, sqlx::Error> {
    let row = sqlx::query!(
        r#"INSERT INTO budget_project (id, budget_id, title, cost_cents, created_at)
           VALUES ($1, $2, $3, $4, $5)
           RETURNING id, budget_id, title, cost_cents, created_at"#,
        id,
        budget_id,
        title,
        cost_cents,
        created_at,
    )
    .fetch_one(executor)
    .await?;
    Ok(ProjectRow {
        id: row.id,
        budget_id: row.budget_id,
        title: row.title,
        cost_cents: row.cost_cents,
        created_at: row.created_at,
    })
}

/// List a budget's projects with keyset pagination over ascending `id`.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn list_projects(
    executor: impl sqlx::PgExecutor<'_>,
    budget_id: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<ProjectRow>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT id, budget_id, title, cost_cents, created_at
           FROM budget_project
           WHERE budget_id = $1 AND ($2::uuid IS NULL OR id > $2)
           ORDER BY id
           LIMIT $3"#,
        budget_id,
        after,
        limit,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| ProjectRow {
            id: row.id,
            budget_id: row.budget_id,
            title: row.title,
            cost_cents: row.cost_cents,
            created_at: row.created_at,
        })
        .collect())
}

/// Count the projects in a budget (for pagination metadata).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn count_projects(
    executor: impl sqlx::PgExecutor<'_>,
    budget_id: Uuid,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT count(*) AS "count!: i64" FROM budget_project WHERE budget_id = $1"#,
        budget_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(row.count)
}

/// Fetch the `(id, cost_cents)` of the given projects **scoped to one budget**, so an order
/// can only allocate to projects that belong to its budget. Projects whose id is not in
/// `project_ids`, or that belong to a different budget, are simply absent from the result —
/// the service compares the returned count against the requested set to detect unknown ids.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn project_costs_in_budget(
    executor: impl sqlx::PgExecutor<'_>,
    budget_id: Uuid,
    project_ids: &[Uuid],
) -> Result<Vec<(Uuid, i64)>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT id, cost_cents
           FROM budget_project
           WHERE budget_id = $1 AND id = ANY($2)"#,
        budget_id,
        project_ids,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| (row.id, row.cost_cents))
        .collect())
}

/// Insert an order header and return the stored row. A duplicate `(budget_id, citizen_id)`
/// raises a unique violation (the service maps it to [`dsoc_core::Error::Conflict`]).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn insert_order(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    budget_id: Uuid,
    citizen_id: Uuid,
    created_at: DateTime<Utc>,
) -> Result<OrderRow, sqlx::Error> {
    let row = sqlx::query!(
        r#"INSERT INTO budget_order (id, budget_id, citizen_id, created_at)
           VALUES ($1, $2, $3, $4)
           RETURNING id, budget_id, citizen_id, created_at"#,
        id,
        budget_id,
        citizen_id,
        created_at,
    )
    .fetch_one(executor)
    .await?;
    Ok(OrderRow {
        id: row.id,
        budget_id: row.budget_id,
        citizen_id: row.citizen_id,
        created_at: row.created_at,
    })
}

/// Link an order to each allocated project in one statement (`UNNEST`). The composite
/// primary key makes a duplicate `(order_id, project_id)` a unique violation.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn insert_order_items(
    executor: impl sqlx::PgExecutor<'_>,
    order_id: Uuid,
    project_ids: &[Uuid],
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"INSERT INTO budget_order_item (order_id, project_id)
           SELECT $1, pid FROM UNNEST($2::uuid[]) AS pid"#,
        order_id,
        project_ids,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// List the project ids an order allocated to, ascending.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn order_item_project_ids(
    executor: impl sqlx::PgExecutor<'_>,
    order_id: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT project_id
           FROM budget_order_item
           WHERE order_id = $1
           ORDER BY project_id"#,
        order_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(|row| row.project_id).collect())
}

/// List a budget's orders with keyset pagination over ascending `id`.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn list_orders(
    executor: impl sqlx::PgExecutor<'_>,
    budget_id: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<OrderRow>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT id, budget_id, citizen_id, created_at
           FROM budget_order
           WHERE budget_id = $1 AND ($2::uuid IS NULL OR id > $2)
           ORDER BY id
           LIMIT $3"#,
        budget_id,
        after,
        limit,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| OrderRow {
            id: row.id,
            budget_id: row.budget_id,
            citizen_id: row.citizen_id,
            created_at: row.created_at,
        })
        .collect())
}

/// Count the orders placed against a budget (for pagination metadata).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn count_orders(
    executor: impl sqlx::PgExecutor<'_>,
    budget_id: Uuid,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT count(*) AS "count!: i64" FROM budget_order WHERE budget_id = $1"#,
        budget_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(row.count)
}

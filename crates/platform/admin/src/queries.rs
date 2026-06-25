//! All persistence for `dsoc-admin`. Every statement is an explicit, compile-time
//! checked `sqlx` query (PLAN.md principle 3) — no ORM, no `SELECT *`, keyset pagination
//! for unbounded reads. These functions return raw row structs and the bare
//! `sqlx::Error`; the service layer maps both to the domain and to `dsoc_core::Error`.

use chrono::{DateTime, Utc};
use dsoc_db::Db;
use uuid::Uuid;

/// Raw `admin_org` row.
pub(crate) struct AdminOrgRow {
    pub org_id: Uuid,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
}

/// Raw `admin_role_binding` row (`role` is the stored text form).
pub(crate) struct RoleBindingRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub citizen_id: Uuid,
    pub role: String,
    pub created_at: DateTime<Utc>,
}

/// Raw `admin_feature_flag` row.
pub(crate) struct FeatureFlagRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub key: String,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Insert the administrative extension for an existing baseline org.
pub(crate) async fn insert_admin_org(
    db: &Db,
    org_id: Uuid,
    created_at: DateTime<Utc>,
) -> Result<AdminOrgRow, sqlx::Error> {
    sqlx::query_as!(
        AdminOrgRow,
        r#"
        INSERT INTO admin_org (org_id, is_active, created_at)
        VALUES ($1, true, $2)
        RETURNING org_id, is_active, created_at
        "#,
        org_id,
        created_at,
    )
    .fetch_one(db)
    .await
}

/// Fetch a single administrative org by its baseline org id.
pub(crate) async fn get_admin_org(db: &Db, org_id: Uuid) -> Result<AdminOrgRow, sqlx::Error> {
    sqlx::query_as!(
        AdminOrgRow,
        r#"
        SELECT org_id, is_active, created_at
        FROM admin_org
        WHERE org_id = $1
        "#,
        org_id,
    )
    .fetch_one(db)
    .await
}

/// List administrative orgs with keyset pagination over `org_id`.
pub(crate) async fn list_admin_orgs(
    db: &Db,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<AdminOrgRow>, sqlx::Error> {
    sqlx::query_as!(
        AdminOrgRow,
        r#"
        SELECT org_id, is_active, created_at
        FROM admin_org
        WHERE ($1::uuid IS NULL OR org_id > $1)
        ORDER BY org_id
        LIMIT $2
        "#,
        after,
        limit,
    )
    .fetch_all(db)
    .await
}

/// Insert a role binding. A duplicate `(org_id, citizen_id, role)` raises a unique
/// violation, which the service maps to [`dsoc_core::Error::Conflict`].
pub(crate) async fn insert_role_binding(
    db: &Db,
    id: Uuid,
    org_id: Uuid,
    citizen_id: Uuid,
    role: &str,
    created_at: DateTime<Utc>,
) -> Result<RoleBindingRow, sqlx::Error> {
    sqlx::query_as!(
        RoleBindingRow,
        r#"
        INSERT INTO admin_role_binding (id, org_id, citizen_id, role, created_at)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, org_id, citizen_id, role, created_at
        "#,
        id,
        org_id,
        citizen_id,
        role,
        created_at,
    )
    .fetch_one(db)
    .await
}

/// List role bindings for one org with keyset pagination over `id`.
pub(crate) async fn list_role_bindings(
    db: &Db,
    org_id: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<RoleBindingRow>, sqlx::Error> {
    sqlx::query_as!(
        RoleBindingRow,
        r#"
        SELECT id, org_id, citizen_id, role, created_at
        FROM admin_role_binding
        WHERE org_id = $1 AND ($2::uuid IS NULL OR id > $2)
        ORDER BY id
        LIMIT $3
        "#,
        org_id,
        after,
        limit,
    )
    .fetch_all(db)
    .await
}

/// Upsert a feature flag so toggling is idempotent: the unique `(org_id, key)` target
/// is updated in place, preserving `id`/`created_at` and advancing `updated_at`.
pub(crate) async fn upsert_feature_flag(
    db: &Db,
    id: Uuid,
    org_id: Uuid,
    key: &str,
    enabled: bool,
    now: DateTime<Utc>,
) -> Result<FeatureFlagRow, sqlx::Error> {
    sqlx::query_as!(
        FeatureFlagRow,
        r#"
        INSERT INTO admin_feature_flag (id, org_id, key, enabled, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, $5)
        ON CONFLICT (org_id, key)
        DO UPDATE SET enabled = EXCLUDED.enabled, updated_at = EXCLUDED.updated_at
        RETURNING id, org_id, key, enabled, created_at, updated_at
        "#,
        id,
        org_id,
        key,
        enabled,
        now,
    )
    .fetch_one(db)
    .await
}

/// Fetch a single feature flag by `(org_id, key)`.
pub(crate) async fn get_feature_flag(
    db: &Db,
    org_id: Uuid,
    key: &str,
) -> Result<FeatureFlagRow, sqlx::Error> {
    sqlx::query_as!(
        FeatureFlagRow,
        r#"
        SELECT id, org_id, key, enabled, created_at, updated_at
        FROM admin_feature_flag
        WHERE org_id = $1 AND key = $2
        "#,
        org_id,
        key,
    )
    .fetch_one(db)
    .await
}

/// List feature flags for one org with keyset pagination over `id`.
pub(crate) async fn list_feature_flags(
    db: &Db,
    org_id: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<FeatureFlagRow>, sqlx::Error> {
    sqlx::query_as!(
        FeatureFlagRow,
        r#"
        SELECT id, org_id, key, enabled, created_at, updated_at
        FROM admin_feature_flag
        WHERE org_id = $1 AND ($2::uuid IS NULL OR id > $2)
        ORDER BY id
        LIMIT $3
        "#,
        org_id,
        after,
        limit,
    )
    .fetch_all(db)
    .await
}

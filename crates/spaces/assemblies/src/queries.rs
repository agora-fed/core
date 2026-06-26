//! Every `sqlx` statement the crate runs, in one place and compile-time checked (PLAN.md
//! principle 3 — explicit, auditable SQL; no ORM, no `SELECT *`, keyset pagination for lists).
//! Functions return raw `sqlx::Error`; [`crate::service`] maps it onto the canonical
//! [`dsoc_core::Error`] model.

use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

/// An assembly row (the permanent participatory body). No `SELECT *` — only the columns surfaced.
#[derive(Debug, Clone)]
pub(crate) struct AssemblyRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

/// A roster row binding a citizen to an assembly with a role.
#[derive(Debug, Clone)]
pub(crate) struct MemberRow {
    pub id: Uuid,
    pub assembly_id: Uuid,
    pub citizen_id: Uuid,
    pub role: String,
    pub joined_at: DateTime<Utc>,
}

/// Persist a fresh assembly. Returns the REAL stored row (RETURNING) — never a phantom id.
pub(crate) async fn insert_assembly<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    org_id: Uuid,
    name: &str,
    created_at: DateTime<Utc>,
) -> Result<AssemblyRow, sqlx::Error> {
    let row = sqlx::query_as!(
        AssemblyRow,
        r#"
        INSERT INTO assembly (id, org_id, name, created_at)
        VALUES ($1, $2, $3, $4)
        RETURNING id, org_id, name, created_at
        "#,
        id,
        org_id,
        name,
        created_at,
    )
    .fetch_one(exec)
    .await?;
    Ok(row)
}

/// Find an assembly within an organization (read for display / membership operations).
pub(crate) async fn find_assembly<'e, E: PgExecutor<'e>>(
    exec: E,
    org_id: Uuid,
    assembly_id: Uuid,
) -> Result<Option<AssemblyRow>, sqlx::Error> {
    let row = sqlx::query_as!(
        AssemblyRow,
        r#"
        SELECT id, org_id, name, created_at
        FROM assembly
        WHERE org_id = $1 AND id = $2
        "#,
        org_id,
        assembly_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

/// Whether an assembly exists within an organization (the [`dsoc_core::traits::Space::ensure_open`]
/// check — an assembly id doubles as the logical `SpaceId` of its participation space).
pub(crate) async fn assembly_exists<'e, E: PgExecutor<'e>>(
    exec: E,
    org_id: Uuid,
    assembly_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM assembly WHERE org_id = $1 AND id = $2) AS "exists!""#,
        org_id,
        assembly_id,
    )
    .fetch_one(exec)
    .await?;
    Ok(exists)
}

/// Add a citizen to an assembly's roster. Idempotent: on the `(assembly_id, citizen_id)` UNIQUE
/// conflict it inserts nothing and returns `None`, leaving the existing row (and its role)
/// untouched — the caller then re-selects the real row with [`find_member`]. Returns the stored
/// row on a fresh insert.
pub(crate) async fn insert_member<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    assembly_id: Uuid,
    citizen_id: Uuid,
    role: &str,
    joined_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
) -> Result<Option<MemberRow>, sqlx::Error> {
    let row = sqlx::query_as!(
        MemberRow,
        r#"
        INSERT INTO assembly_member
            (id, assembly_id, citizen_id, role, joined_at, created_at)
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (assembly_id, citizen_id) DO NOTHING
        RETURNING id, assembly_id, citizen_id, role, joined_at
        "#,
        id,
        assembly_id,
        citizen_id,
        role,
        joined_at,
        created_at,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

/// Re-select an existing membership by its natural key (used after an idempotent no-op insert so
/// the caller always gets the REAL stored row id, never a phantom one).
pub(crate) async fn find_member<'e, E: PgExecutor<'e>>(
    exec: E,
    assembly_id: Uuid,
    citizen_id: Uuid,
) -> Result<Option<MemberRow>, sqlx::Error> {
    let row = sqlx::query_as!(
        MemberRow,
        r#"
        SELECT id, assembly_id, citizen_id, role, joined_at
        FROM assembly_member
        WHERE assembly_id = $1 AND citizen_id = $2
        "#,
        assembly_id,
        citizen_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

/// Keyset-paginated roster for an assembly, ordered by member id ascending. The cursor is the last
/// id of the previous page; pass `None` for the first page.
pub(crate) async fn list_members<'e, E: PgExecutor<'e>>(
    exec: E,
    assembly_id: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<MemberRow>, sqlx::Error> {
    let rows = sqlx::query_as!(
        MemberRow,
        r#"
        SELECT id, assembly_id, citizen_id, role, joined_at
        FROM assembly_member
        WHERE assembly_id = $1
          AND ($2::uuid IS NULL OR id > $2)
        ORDER BY id ASC
        LIMIT $3
        "#,
        assembly_id,
        after,
        limit,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

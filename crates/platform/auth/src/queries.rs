//! Every `sqlx` statement the crate runs, in one place and compile-time checked (PLAN.md
//! principle 3 — explicit, auditable SQL; no ORM, no `SELECT *`, keyset pagination for lists).
//! Functions return raw `sqlx::Error`; [`crate::service`] maps it onto the canonical
//! [`dsoc_core::Error`] model.

use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

/// A citizen row read for identity resolution (core-owned `citizen` table).
#[derive(Debug, Clone)]
pub(crate) struct CitizenRow {
    pub id: Uuid,
    pub verification_level: String,
}

/// A row of the verification-level audit trail.
#[derive(Debug, Clone)]
pub(crate) struct AuditRow {
    pub id: Uuid,
    pub old_level: String,
    pub new_level: String,
    pub changed_at: DateTime<Utc>,
}

/// Find the citizen bound to an OIDC subject within an organization.
pub(crate) async fn find_citizen_by_subject<'e, E: PgExecutor<'e>>(
    exec: E,
    org_id: Uuid,
    oidc_subject: &str,
) -> Result<Option<CitizenRow>, sqlx::Error> {
    let row = sqlx::query_as!(
        CitizenRow,
        r#"
        SELECT id, verification_level
        FROM citizen
        WHERE org_id = $1 AND oidc_subject = $2
        "#,
        org_id,
        oidc_subject,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

/// Provision a new citizen at first sovereign login (verified to `level`).
pub(crate) async fn insert_citizen<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    org_id: Uuid,
    oidc_subject: &str,
    level: &str,
    created_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO citizen (id, org_id, oidc_subject, verification_level, created_at)
        VALUES ($1, $2, $3, $4, $5)
        "#,
        id,
        org_id,
        oidc_subject,
        level,
        created_at,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Read a citizen's current verification level (the core of [`dsoc_core::Authorization::level`]).
pub(crate) async fn citizen_level<'e, E: PgExecutor<'e>>(
    exec: E,
    org_id: Uuid,
    citizen_id: Uuid,
) -> Result<String, sqlx::Error> {
    let level = sqlx::query_scalar!(
        r#"
        SELECT verification_level
        FROM citizen
        WHERE org_id = $1 AND id = $2
        "#,
        org_id,
        citizen_id,
    )
    .fetch_one(exec)
    .await?;
    Ok(level)
}

/// Read a citizen's current verification level **and lock the row** (`FOR UPDATE`) for the rest of
/// the caller's transaction. Two concurrent upgrades for the same citizen serialize on this lock,
/// so only one can append to the append-only audit trail per genuine transition (closes the TOCTOU
/// where the level was read outside the transaction). [`citizen_level`] is the lock-free read.
pub(crate) async fn citizen_level_for_update<'e, E: PgExecutor<'e>>(
    exec: E,
    org_id: Uuid,
    citizen_id: Uuid,
) -> Result<String, sqlx::Error> {
    let level = sqlx::query_scalar!(
        r#"
        SELECT verification_level
        FROM citizen
        WHERE org_id = $1 AND id = $2
        FOR UPDATE
        "#,
        org_id,
        citizen_id,
    )
    .fetch_one(exec)
    .await?;
    Ok(level)
}

/// Advance a citizen's verification level, guarded by the level we expect it to still hold
/// (optimistic concurrency). The `AND verification_level = $3` predicate means a lost race returns
/// 0 rows affected instead of clobbering a concurrent writer. Runs in the upgrade transaction
/// alongside the audit insert.
pub(crate) async fn update_citizen_level<'e, E: PgExecutor<'e>>(
    exec: E,
    org_id: Uuid,
    citizen_id: Uuid,
    expected_level: &str,
    new_level: &str,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query!(
        r#"
        UPDATE citizen
        SET verification_level = $4
        WHERE org_id = $1 AND id = $2 AND verification_level = $3
        "#,
        org_id,
        citizen_id,
        expected_level,
        new_level,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected())
}

/// Append an immutable verification-level change to the audit trail.
pub(crate) async fn insert_verification_audit<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    org_id: Uuid,
    citizen_id: Uuid,
    old_level: &str,
    new_level: &str,
    changed_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO auth_verification_level
            (id, org_id, citizen_id, old_level, new_level, changed_at)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
        id,
        org_id,
        citizen_id,
        old_level,
        new_level,
        changed_at,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Persist a freshly-issued session (the ActivityPub keypair seam stays NULL — ADR-0005).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn insert_session<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    org_id: Uuid,
    citizen_id: Uuid,
    oidc_subject: &str,
    issued_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    public_handle: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO auth_session
            (id, org_id, citizen_id, oidc_subject, issued_at, expires_at, public_handle, created_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $5)
        "#,
        id,
        org_id,
        citizen_id,
        oidc_subject,
        issued_at,
        expires_at,
        public_handle,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Keyset-paginated verification-level history for a citizen, newest first. The cursor is the
/// `(changed_at, id)` of the last row of the previous page; pass `None` for the first page.
pub(crate) async fn verification_history<'e, E: PgExecutor<'e>>(
    exec: E,
    citizen_id: Uuid,
    cursor: Option<(DateTime<Utc>, Uuid)>,
    limit: i64,
) -> Result<Vec<AuditRow>, sqlx::Error> {
    let (cursor_at, cursor_id) = match cursor {
        Some((at, id)) => (Some(at), Some(id)),
        None => (None, None),
    };
    let rows = sqlx::query_as!(
        AuditRow,
        r#"
        SELECT id, old_level, new_level, changed_at
        FROM auth_verification_level
        WHERE citizen_id = $1
          AND ($2::timestamptz IS NULL OR (changed_at, id) < ($2, $3))
        ORDER BY changed_at DESC, id DESC
        LIMIT $4
        "#,
        citizen_id,
        cursor_at,
        cursor_id,
        limit,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

/// Minimal credential lookup row for login.
#[derive(Debug)]
pub(crate) struct CredentialRow {
    pub citizen_id: Uuid,
    pub password_hash: String,
}

/// Insert a credential-authenticated citizen (no external OIDC subject).
pub(crate) async fn insert_credential_citizen<'e, E: PgExecutor<'e>>(
    ex: E,
    id: Uuid,
    org: Uuid,
    level: &str,
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "INSERT INTO citizen (id, org_id, oidc_subject, verification_level, created_at) \
         VALUES ($1, $2, NULL, $3, $4)",
        id,
        org,
        level,
        now
    )
    .execute(ex)
    .await?;
    Ok(())
}

/// Insert the e-mail/senha/CPF credential for a citizen.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn insert_credential<'e, E: PgExecutor<'e>>(
    ex: E,
    id: Uuid,
    citizen: Uuid,
    org: Uuid,
    email: &str,
    password_hash: &str,
    cpf: &str,
    cpf_status: &str,
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "INSERT INTO auth_credential \
         (id, citizen_id, org_id, email, password_hash, cpf, cpf_status, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        id,
        citizen,
        org,
        email,
        password_hash,
        cpf,
        cpf_status,
        now
    )
    .execute(ex)
    .await?;
    Ok(())
}

/// Look up a credential by e-mail within an org (for login).
pub(crate) async fn find_credential_by_email<'e, E: PgExecutor<'e>>(
    ex: E,
    org: Uuid,
    email: &str,
) -> Result<Option<CredentialRow>, sqlx::Error> {
    let row = sqlx::query!(
        "SELECT citizen_id, password_hash FROM auth_credential WHERE org_id = $1 AND email = $2",
        org,
        email
    )
    .fetch_optional(ex)
    .await?;
    Ok(row.map(|r| CredentialRow {
        citizen_id: r.citizen_id,
        password_hash: r.password_hash,
    }))
}

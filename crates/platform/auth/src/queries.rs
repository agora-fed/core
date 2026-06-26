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

/// Resolve a live (non-expired) session to its (citizen_id, org_id) — used by the gateway auth
/// middleware to turn the session cookie into the caller's identity.
pub async fn session_identity<'e, E: PgExecutor<'e>>(
    ex: E,
    session_id: Uuid,
) -> Result<Option<(Uuid, Uuid)>, sqlx::Error> {
    let row = sqlx::query!(
        "SELECT citizen_id, org_id FROM auth_session WHERE id = $1 AND expires_at > now()",
        session_id
    )
    .fetch_optional(ex)
    .await?;
    Ok(row.map(|r| (r.citizen_id, r.org_id)))
}

/// Delete a session row by id (logout). Idempotent: a missing row reports 0 affected and is not
/// an error — a logout request on an already-expired or cleared session still succeeds at the
/// HTTP layer so a stale tab never sees an error trying to sign out.
pub async fn delete_session<'e, E: PgExecutor<'e>>(
    ex: E,
    session_id: Uuid,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query!("DELETE FROM auth_session WHERE id = $1", session_id)
        .execute(ex)
        .await?;
    Ok(result.rows_affected())
}

/// The full profile row read for `GET /me` / federation Actor materialization (ADR-0010). Carries
/// no credentials — CPF and password hash live in `auth_credential` and never escape the service
/// boundary, regardless of how the caller queries.
#[derive(Debug, Clone)]
pub(crate) struct ProfileRow {
    pub citizen_id: Uuid,
    pub org_id: Uuid,
    pub handle: Option<String>,
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub avatar_object_key: Option<String>,
    pub cover_object_key: Option<String>,
    pub is_public: bool,
    pub verification_level: String,
    pub created_at: DateTime<Utc>,
}

/// Read a citizen's profile by id. Returns `None` for a missing/deleted citizen.
pub(crate) async fn find_profile<'e, E: PgExecutor<'e>>(
    ex: E,
    citizen_id: Uuid,
) -> Result<Option<ProfileRow>, sqlx::Error> {
    let row = sqlx::query_as!(
        ProfileRow,
        r#"
        SELECT id AS citizen_id,
               org_id,
               handle,
               display_name,
               bio,
               avatar_object_key,
               cover_object_key,
               is_public,
               verification_level,
               created_at
          FROM citizen
         WHERE id = $1
        "#,
        citizen_id,
    )
    .fetch_optional(ex)
    .await?;
    Ok(row)
}

/// Read the citizen's current avatar/cover keys (before an update). Two-step (read + update)
/// avoids the SQL gymnastics needed to coax the pre-update value out of `RETURNING`; the race
/// window between the two statements only matters for the same citizen uploading two avatars
/// within microseconds (harmless — worst case one orphan object in storage).
pub(crate) async fn current_media_keys<'e, E: PgExecutor<'e>>(
    ex: E,
    citizen_id: Uuid,
) -> Result<Option<(Option<String>, Option<String>)>, sqlx::Error> {
    let row = sqlx::query!(
        "SELECT avatar_object_key, cover_object_key FROM citizen WHERE id = $1",
        citizen_id,
    )
    .fetch_optional(ex)
    .await?;
    Ok(row.map(|r| (r.avatar_object_key, r.cover_object_key)))
}

/// Update the citizen's avatar object key (the bytes already live in S3/MinIO at `key`).
pub(crate) async fn update_avatar_object_key<'e, E: PgExecutor<'e>>(
    ex: E,
    citizen_id: Uuid,
    new_key: &str,
) -> Result<u64, sqlx::Error> {
    let r = sqlx::query!(
        "UPDATE citizen SET avatar_object_key = $2, profile_updated_at = now() WHERE id = $1",
        citizen_id,
        new_key,
    )
    .execute(ex)
    .await?;
    Ok(r.rows_affected())
}

/// Same shape as [`update_avatar_object_key`] but for the cover image.
pub(crate) async fn update_cover_object_key<'e, E: PgExecutor<'e>>(
    ex: E,
    citizen_id: Uuid,
    new_key: &str,
) -> Result<u64, sqlx::Error> {
    let r = sqlx::query!(
        "UPDATE citizen SET cover_object_key = $2, profile_updated_at = now() WHERE id = $1",
        citizen_id,
        new_key,
    )
    .execute(ex)
    .await?;
    Ok(r.rows_affected())
}

/// One row from `auth_password_reset` (live or used). Carries the citizen id needed to update
/// the matching credential on confirmation.
#[derive(Debug, Clone)]
pub(crate) struct PasswordResetRow {
    pub id: Uuid,
    pub citizen_id: Uuid,
}

/// Insert a fresh reset row. The caller pre-computes the SHA-256 of the plaintext token; only
/// the hash lives in the column.
pub(crate) async fn password_reset_insert<'e, E: PgExecutor<'e>>(
    ex: E,
    id: Uuid,
    citizen_id: Uuid,
    token_hash: &[u8],
    expires_at: DateTime<Utc>,
    request_ip: Option<&str>,
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO auth_password_reset (id, citizen_id, token_hash, expires_at, used_at, request_ip, created_at)
        VALUES ($1, $2, $3, $4, NULL, $5, $6)
        "#,
        id,
        citizen_id,
        token_hash,
        expires_at,
        request_ip,
        now,
    )
    .execute(ex)
    .await?;
    Ok(())
}

/// Invalidate any LIVE reset row for a citizen by stamping `used_at = now`. Called before
/// inserting a fresh request so at most one link is live at a time per citizen.
pub(crate) async fn password_reset_invalidate_live<'e, E: PgExecutor<'e>>(
    ex: E,
    citizen_id: Uuid,
    now: DateTime<Utc>,
) -> Result<u64, sqlx::Error> {
    let r = sqlx::query!(
        r#"
        UPDATE auth_password_reset
           SET used_at = $2
         WHERE citizen_id = $1 AND used_at IS NULL
        "#,
        citizen_id,
        now,
    )
    .execute(ex)
    .await?;
    Ok(r.rows_affected())
}

/// Look up a redeemable reset row by token hash + an `expires_at > now` guard. Returns `None`
/// for unknown / expired / already-used tokens (the surface never tells the caller which).
pub(crate) async fn password_reset_find_live<'e, E: PgExecutor<'e>>(
    ex: E,
    token_hash: &[u8],
    now: DateTime<Utc>,
) -> Result<Option<PasswordResetRow>, sqlx::Error> {
    let row = sqlx::query_as!(
        PasswordResetRow,
        r#"
        SELECT id, citizen_id
          FROM auth_password_reset
         WHERE token_hash = $1 AND used_at IS NULL AND expires_at > $2
        "#,
        token_hash,
        now,
    )
    .fetch_optional(ex)
    .await?;
    Ok(row)
}

/// Mark a reset row as used (single-use enforcement). Called inside the confirm transaction
/// alongside the credential password update; if the tx rolls back the row stays live.
pub(crate) async fn password_reset_mark_used<'e, E: PgExecutor<'e>>(
    ex: E,
    id: Uuid,
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE auth_password_reset SET used_at = $2 WHERE id = $1",
        id,
        now,
    )
    .execute(ex)
    .await?;
    Ok(())
}

/// Swap the credential's password hash. Re-uses the existing `auth_credential` row keyed by
/// `citizen_id` (1-1 because `auth_credential.citizen_id` is UNIQUE per migration 0101).
pub(crate) async fn credential_update_password<'e, E: PgExecutor<'e>>(
    ex: E,
    citizen_id: Uuid,
    new_hash: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE auth_credential SET password_hash = $2 WHERE citizen_id = $1",
        citizen_id,
        new_hash,
    )
    .execute(ex)
    .await?;
    Ok(())
}

/// Delete every session belonging to a citizen — used after a password reset so a leaked
/// credential cannot leave a foothold via a still-valid cookie.
pub(crate) async fn delete_all_sessions_for_citizen<'e, E: PgExecutor<'e>>(
    ex: E,
    citizen_id: Uuid,
) -> Result<u64, sqlx::Error> {
    let r = sqlx::query!(
        "DELETE FROM auth_session WHERE citizen_id = $1",
        citizen_id,
    )
    .execute(ex)
    .await?;
    Ok(r.rows_affected())
}

/// Patch a citizen's profile. `None` arguments leave the column untouched (`COALESCE`); an
/// explicit `Some("")` for nullable text fields collapses to `NULL` so a citizen can clear a bio
/// or display name. Updates `profile_updated_at = now()` so the federation crate (W2) can detect
/// when to refresh remote inboxes' cached Actor object.
pub(crate) async fn update_profile<'e, E: PgExecutor<'e>>(
    ex: E,
    citizen_id: Uuid,
    display_name: Option<Option<String>>,
    bio: Option<Option<String>>,
    handle: Option<Option<String>>,
    is_public: Option<bool>,
) -> Result<Option<ProfileRow>, sqlx::Error> {
    // Two-level Option lets us distinguish "leave untouched" (outer None) from "set to NULL"
    // (Some(None)) from "set to value" (Some(Some(v))). Flatten + an `apply` flag per column for
    // the SQL — each column is COALESCE($value, current) when not applied, else just $value.
    let (set_display, display_value) = match display_name {
        Some(v) => (true, v),
        None => (false, None),
    };
    let (set_bio, bio_value) = match bio {
        Some(v) => (true, v),
        None => (false, None),
    };
    let (set_handle, handle_value) = match handle {
        Some(v) => (true, v),
        None => (false, None),
    };
    let (set_is_public, is_public_value) = match is_public {
        Some(v) => (true, v),
        None => (false, false),
    };

    let row = sqlx::query_as!(
        ProfileRow,
        r#"
        UPDATE citizen
           SET display_name       = CASE WHEN $2 THEN NULLIF($3, '') ELSE display_name END,
               bio                = CASE WHEN $4 THEN NULLIF($5, '') ELSE bio END,
               handle             = CASE WHEN $6 THEN NULLIF($7, '') ELSE handle END,
               is_public          = CASE WHEN $8 THEN $9 ELSE is_public END,
               profile_updated_at = now()
         WHERE id = $1
        RETURNING id AS citizen_id,
                  org_id,
                  handle,
                  display_name,
                  bio,
                  avatar_object_key,
                  cover_object_key,
                  is_public,
                  verification_level,
                  created_at
        "#,
        citizen_id,
        set_display,
        display_value,
        set_bio,
        bio_value,
        set_handle,
        handle_value,
        set_is_public,
        is_public_value,
    )
    .fetch_optional(ex)
    .await?;
    Ok(row)
}

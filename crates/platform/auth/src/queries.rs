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

/// Read the `public_email` of a mandate within an org. Returns `None` if the mandate does
/// not exist. Used by [`crate::service::ZitadelAuth::register_politician`] to gate the
/// self-registration flow: a citizen may only self-onboard as a politician when they can
/// prove control of the `public_email` on file with the Câmara/Senado/TSE.
pub(crate) async fn find_mandate_public_email<'e, E: PgExecutor<'e>>(
    ex: E,
    org: Uuid,
    mandate: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query_scalar!(
        "SELECT public_email FROM mandate WHERE org_id = $1 AND id = $2",
        org,
        mandate,
    )
    .fetch_optional(ex)
    .await?;
    Ok(row)
}

/// Force `is_public=true` on a citizen row. Called by the politician self-registration flow
/// — mandate operators are always public (accountability transparency; not opt-out).
pub(crate) async fn force_citizen_public<'e, E: PgExecutor<'e>>(
    ex: E,
    citizen: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE citizen SET is_public = true, profile_updated_at = now() WHERE id = $1",
        citizen,
    )
    .execute(ex)
    .await?;
    Ok(())
}

/// Insert a mandate_identity_binding row. Written by the politician self-registration flow
/// so the `Painel do mandato` (F1+F3) resolves the new citizen back to their SLAs on first
/// login. `verification_level` is normally `directory` here (e-mail match to public_email).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn insert_mandate_identity_binding<'e, E: PgExecutor<'e>>(
    ex: E,
    id: Uuid,
    mandate: Uuid,
    citizen: Uuid,
    verification_level: &str,
    evidence_ref: Option<&str>,
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "INSERT INTO mandate_identity_binding \
         (id, mandate_id, citizen_id, verification_level, evidence_ref, verified_at, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $6)",
        id,
        mandate,
        citizen,
        verification_level,
        evidence_ref,
        now,
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

/// One session row as listed for the citizen on `GET /me/sessions`. Carries no credentials —
/// the session id IS the credential (the cookie) so we surface only what the UI needs to render
/// (timestamps) and what the revoke surface needs (the id).
#[derive(Debug, Clone)]
pub(crate) struct SessionListRow {
    pub id: Uuid,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// List the live (non-expired) sessions of a citizen, newest first. Expired rows are excluded so
/// the surface matches what is actually usable; cleanup of dead rows is a separate concern
/// (could be added to the SLA sweeper if it becomes a volume issue).
pub(crate) async fn list_sessions_for_citizen<'e, E: PgExecutor<'e>>(
    ex: E,
    citizen_id: Uuid,
    now: DateTime<Utc>,
) -> Result<Vec<SessionListRow>, sqlx::Error> {
    let rows = sqlx::query_as!(
        SessionListRow,
        r#"
        SELECT id, issued_at, expires_at
          FROM auth_session
         WHERE citizen_id = $1 AND expires_at > $2
         ORDER BY issued_at DESC, id DESC
        "#,
        citizen_id,
        now,
    )
    .fetch_all(ex)
    .await?;
    Ok(rows)
}

/// Revoke a session BUT ONLY if it belongs to the given citizen (the WHERE clause is the
/// security boundary — without `AND citizen_id = $2` a logged-in user could revoke other users'
/// sessions by guessing their ids). Returns the number of rows affected so the service can
/// distinguish "deleted" from "wasn't yours / doesn't exist".
pub(crate) async fn delete_session_for_citizen<'e, E: PgExecutor<'e>>(
    ex: E,
    session_id: Uuid,
    citizen_id: Uuid,
) -> Result<u64, sqlx::Error> {
    let r = sqlx::query!(
        "DELETE FROM auth_session WHERE id = $1 AND citizen_id = $2",
        session_id,
        citizen_id,
    )
    .execute(ex)
    .await?;
    Ok(r.rows_affected())
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
    pub titulo_status: Option<String>,
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
               titulo_status,
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

/// Read the PRIVATE PEM of a citizen's federation actor key. Used by the outbound delivery
/// path (signing Accept, Create etc.). NEVER returned by any HTTP surface — this function is
/// internal to the platform tier. Returns `None` when the citizen has no key yet (lazy gen
/// hasn't run because they're not public or never had a federation hit).
pub(crate) async fn find_actor_private_key<'e, E: PgExecutor<'e>>(
    ex: E,
    citizen_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query!(
        "SELECT private_pem FROM citizen_actor_key WHERE citizen_id = $1",
        citizen_id,
    )
    .fetch_optional(ex)
    .await?;
    Ok(row.map(|r| r.private_pem))
}

/// Try to record an inbound activity id as "seen" before acting on it. Returns `true` for a
/// FRESH delivery (the caller should act), `false` for a duplicate (the caller should reply
/// 202 immediately without re-acting). The INSERT-then-check pattern is the strict idempotency
/// guarantee Mastodon retries against.
pub(crate) async fn mark_inbox_activity_seen<'e, E: PgExecutor<'e>>(
    ex: E,
    activity_id: &str,
    citizen_id: Uuid,
    now: DateTime<Utc>,
) -> Result<bool, sqlx::Error> {
    let r = sqlx::query!(
        r#"
        INSERT INTO federation_inbox_seen (activity_id, citizen_id, seen_at)
        VALUES ($1, $2, $3)
        ON CONFLICT (activity_id) DO NOTHING
        "#,
        activity_id,
        citizen_id,
        now,
    )
    .execute(ex)
    .await?;
    Ok(r.rows_affected() == 1)
}

/// Persist an inbound follow (someone remote follows our citizen). Idempotent at the schema
/// level (unique on (citizen_id, direction, remote_actor_url)); a duplicate inbound Follow
/// from the same remote actor is a no-op.
pub(crate) async fn insert_inbound_follow<'e, E: PgExecutor<'e>>(
    ex: E,
    id: Uuid,
    citizen_id: Uuid,
    remote_actor_url: &str,
    remote_inbox_url: &str,
    follow_activity_id: &str,
    now: DateTime<Utc>,
) -> Result<u64, sqlx::Error> {
    let r = sqlx::query!(
        r#"
        INSERT INTO federation_follow
            (id, citizen_id, direction, remote_actor_url, remote_inbox_url,
             follow_activity_id, accepted_at, created_at)
        VALUES ($1, $2, 'inbound', $3, $4, $5, NULL, $6)
        ON CONFLICT (citizen_id, direction, remote_actor_url) DO UPDATE
            SET remote_inbox_url    = EXCLUDED.remote_inbox_url,
                follow_activity_id  = EXCLUDED.follow_activity_id
        "#,
        id,
        citizen_id,
        remote_actor_url,
        remote_inbox_url,
        follow_activity_id,
        now,
    )
    .execute(ex)
    .await?;
    Ok(r.rows_affected())
}

/// Mark an inbound follow as ACK'd (we successfully delivered the Accept back). Lookup is by
/// the natural key — (citizen, direction, remote actor) — not the surrogate id.
pub(crate) async fn mark_inbound_follow_accepted<'e, E: PgExecutor<'e>>(
    ex: E,
    citizen_id: Uuid,
    remote_actor_url: &str,
    now: DateTime<Utc>,
) -> Result<u64, sqlx::Error> {
    let r = sqlx::query!(
        r#"
        UPDATE federation_follow
           SET accepted_at = $3
         WHERE citizen_id = $1
           AND direction = 'inbound'
           AND remote_actor_url = $2
        "#,
        citizen_id,
        remote_actor_url,
        now,
    )
    .execute(ex)
    .await?;
    Ok(r.rows_affected())
}

/// Page of ACK'd inbound followers of a citizen. Each row is one remote actor URL. Used by the
/// `GET /actors/{handle}/followers` OrderedCollection (Mastodon reads this for the count).
pub(crate) async fn list_inbound_followers<'e, E: PgExecutor<'e>>(
    ex: E,
    citizen_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query_scalar!(
        r#"
        SELECT remote_actor_url
          FROM federation_follow
         WHERE citizen_id = $1
           AND direction = 'inbound'
           AND accepted_at IS NOT NULL
         ORDER BY created_at DESC
         LIMIT $2 OFFSET $3
        "#,
        citizen_id,
        limit,
        offset,
    )
    .fetch_all(ex)
    .await?;
    Ok(rows)
}

// --- W2.5: outbox + delivery -----------------------------------------------------------------

/// One claimable delivery row pulled by the worker. Carries the actor's signing key + the
/// recipient inbox + the body to POST, so the worker doesn't need to JOIN per row.
#[derive(Debug, Clone)]
pub(crate) struct DeliveryClaim {
    pub delivery_id: Uuid,
    pub recipient_inbox: String,
    pub actor_url: String,
    pub private_pem: String,
    pub payload: serde_json::Value,
    pub attempts: i32,
}

/// Insert a new outbox entry (the wire-ready Activity). Returns the entry id so the caller
/// can chain it into the per-follower delivery fanout.
///
/// 0.18.0: runtime unchecked so we can widen the column set (in_reply_to_uri, sensitive,
/// spoiler_text) without regenerating the `.sqlx/` offline cache.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn insert_outbox_entry<'e, E: PgExecutor<'e>>(
    ex: E,
    id: Uuid,
    citizen_id: Uuid,
    activity_id: &str,
    kind: &str,
    visibility: &str,
    payload: &serde_json::Value,
    now: DateTime<Utc>,
    in_reply_to_uri: Option<&str>,
    sensitive: bool,
    spoiler_text: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r"
        INSERT INTO federation_outbox_entry
            (id, citizen_id, activity_id, kind, visibility, payload, created_at,
             in_reply_to_uri, sensitive, spoiler_text)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        ",
    )
    .bind(id)
    .bind(citizen_id)
    .bind(activity_id)
    .bind(kind)
    .bind(visibility)
    .bind(payload)
    .bind(now)
    .bind(in_reply_to_uri)
    .bind(sensitive)
    .bind(spoiler_text)
    .execute(ex)
    .await?;
    Ok(())
}

/// Persist an extracted hashtag reference (idempotent on `(object_uri, tag_normalized)`).
/// 0.18.0 — populated by both outbound publish and inbound receipt.
pub(crate) async fn insert_note_hashtag<'e, E: PgExecutor<'e>>(
    ex: E,
    object_uri: &str,
    tag_normalized: &str,
    tag_original: &str,
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r"
        INSERT INTO note_hashtag
            (id, object_uri, tag_normalized, tag_original, created_at)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (object_uri, tag_normalized) DO NOTHING
        ",
    )
    .bind(Uuid::now_v7())
    .bind(object_uri)
    .bind(tag_normalized)
    .bind(tag_original)
    .bind(now)
    .execute(ex)
    .await?;
    Ok(())
}

/// Persist an extracted mention reference (idempotent on `(object_uri, mentioned_actor_url)`).
pub(crate) async fn insert_note_mention<'e, E: PgExecutor<'e>>(
    ex: E,
    object_uri: &str,
    mentioned_actor_url: &str,
    mentioned_handle: &str,
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r"
        INSERT INTO note_mention
            (id, object_uri, mentioned_actor_url, mentioned_handle, created_at)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (object_uri, mentioned_actor_url) DO NOTHING
        ",
    )
    .bind(Uuid::now_v7())
    .bind(object_uri)
    .bind(mentioned_actor_url)
    .bind(mentioned_handle)
    .bind(now)
    .execute(ex)
    .await?;
    Ok(())
}

/// Fan out an outbox entry to every ACK'd inbound follower of the citizen. One row per
/// (entry, inbox). Idempotent via the UNIQUE on (outbox_entry_id, recipient_inbox).
pub(crate) async fn fanout_delivery_to_followers<'e, E: PgExecutor<'e>>(
    ex: E,
    outbox_entry_id: Uuid,
    citizen_id: Uuid,
    now: DateTime<Utc>,
) -> Result<u64, sqlx::Error> {
    // Generates one new uuid per row via gen_random_uuid() (pgcrypto already installed for
    // mandate_invitation hashing); avoids streaming the follower list back to the gateway.
    let r = sqlx::query!(
        r#"
        INSERT INTO federation_delivery
            (id, outbox_entry_id, recipient_inbox, attempts, next_attempt_at, delivered_at,
             last_error, created_at)
        SELECT gen_random_uuid(), $1, f.remote_inbox_url, 0, $3, NULL, NULL, $3
          FROM federation_follow f
         WHERE f.citizen_id = $2
           AND f.direction = 'inbound'
           AND f.accepted_at IS NOT NULL
           AND f.remote_inbox_url IS NOT NULL
        ON CONFLICT (outbox_entry_id, recipient_inbox) DO NOTHING
        "#,
        outbox_entry_id,
        citizen_id,
        now,
    )
    .execute(ex)
    .await?;
    Ok(r.rows_affected())
}

/// List a citizen's public outbox entries, newest first. Payload returned as-is so the Outbox
/// endpoint can put the JSONB straight into the OrderedCollection.
pub(crate) async fn list_public_outbox<'e, E: PgExecutor<'e>>(
    ex: E,
    citizen_id: Uuid,
    limit: i64,
) -> Result<Vec<serde_json::Value>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT payload
          FROM federation_outbox_entry
         WHERE citizen_id = $1 AND visibility = 'public'
         ORDER BY created_at DESC
         LIMIT $2
        "#,
        citizen_id,
        limit,
    )
    .fetch_all(ex)
    .await?;
    Ok(rows.into_iter().map(|r| r.payload).collect())
}

pub(crate) async fn count_public_outbox<'e, E: PgExecutor<'e>>(
    ex: E,
    citizen_id: Uuid,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT count(*) AS "n!" FROM federation_outbox_entry WHERE citizen_id = $1 AND visibility = 'public'"#,
        citizen_id,
    )
    .fetch_one(ex)
    .await?;
    Ok(row.n)
}

/// Claim a batch of pending deliveries whose `next_attempt_at <= now`. Uses `FOR UPDATE SKIP
/// LOCKED` so multiple worker tasks (today: one; future: many) never race for the same row.
/// The same statement INCREMENTS `attempts` and pushes `next_attempt_at` far into the future
/// inside the same transaction — so even if the worker crashes mid-delivery, the row won't be
/// re-picked instantly. On success the worker stamps `delivered_at`; on failure it explicitly
/// resets `next_attempt_at` to an exponential-backoff time.
///
/// Returns the claimed rows joined with the actor's signing material and the activity payload.
pub(crate) async fn claim_pending_deliveries(
    db: &dsoc_db::Db,
    now: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<DeliveryClaim>, sqlx::Error> {
    // The CTE locks the rows we'll touch; the outer UPDATE bumps attempts and stages a short
    // pessimistic next_attempt window so a crash before success/failure leaves the row in a
    // sane state (re-claimable after a few minutes).
    let rows = sqlx::query!(
        r#"
        WITH claimed AS (
            SELECT id
              FROM federation_delivery
             WHERE delivered_at IS NULL AND next_attempt_at <= $1
             ORDER BY next_attempt_at, id
             FOR UPDATE SKIP LOCKED
             LIMIT $2
        )
        UPDATE federation_delivery d
           SET attempts        = d.attempts + 1,
               next_attempt_at = $1 + interval '5 minutes'
          FROM claimed
         WHERE d.id = claimed.id
        RETURNING d.id            AS delivery_id,
                  d.recipient_inbox,
                  d.outbox_entry_id,
                  d.attempts
        "#,
        now,
        limit,
    )
    .fetch_all(db)
    .await?;

    if rows.is_empty() {
        return Ok(Vec::new());
    }

    // Second round-trip: pull the payload + actor signing material for each claimed row. Done
    // in a single batched query keyed on the entry ids to avoid N+1.
    let entry_ids: Vec<Uuid> = rows.iter().map(|r| r.outbox_entry_id).collect();
    let entries = sqlx::query!(
        r#"
        SELECT o.id           AS entry_id,
               o.payload      AS payload,
               o.citizen_id   AS citizen_id,
               k.private_pem  AS "private_pem!"
          FROM federation_outbox_entry o
          JOIN citizen_actor_key k ON k.citizen_id = o.citizen_id
         WHERE o.id = ANY($1)
        "#,
        &entry_ids,
    )
    .fetch_all(db)
    .await?;

    // Also need the citizen's handle to build the actor URL. The federation surface uses the
    // user-chosen `handle` (or the opaque public_handle as fallback).
    let citizen_ids: Vec<Uuid> = entries.iter().map(|e| e.citizen_id).collect();
    let citizens = sqlx::query!(
        r#"
        SELECT id AS "id!",
               handle AS "handle"
          FROM citizen
         WHERE id = ANY($1)
        "#,
        &citizen_ids,
    )
    .fetch_all(db)
    .await?;

    let public_origin = std::env::var("PUBLIC_ORIGIN")
        .unwrap_or_else(|_| "https://democracia.social.br".to_owned());
    let public_origin = public_origin.trim_end_matches('/').to_owned();

    let mut claims = Vec::with_capacity(rows.len());
    for r in rows {
        let Some(entry) = entries.iter().find(|e| e.entry_id == r.outbox_entry_id) else {
            continue;
        };
        let Some(c) = citizens.iter().find(|c| c.id == entry.citizen_id) else {
            continue;
        };
        let handle = c
            .handle
            .clone()
            .unwrap_or_else(|| format!("u-{}", c.id.simple()));
        let actor_url = format!("{public_origin}/actors/{handle}");
        claims.push(DeliveryClaim {
            delivery_id: r.delivery_id,
            recipient_inbox: r.recipient_inbox,
            actor_url,
            private_pem: entry.private_pem.clone(),
            payload: entry.payload.clone(),
            attempts: r.attempts,
        });
    }
    Ok(claims)
}

/// Mark a delivery as successful (the worker just got a 2xx). The row becomes inert.
pub(crate) async fn mark_delivery_done<'e, E: PgExecutor<'e>>(
    ex: E,
    delivery_id: Uuid,
    now: DateTime<Utc>,
) -> Result<u64, sqlx::Error> {
    let r = sqlx::query!(
        "UPDATE federation_delivery SET delivered_at = $2, last_error = NULL WHERE id = $1",
        delivery_id,
        now,
    )
    .execute(ex)
    .await?;
    Ok(r.rows_affected())
}

/// Record a failed delivery and schedule the next attempt. Exponential backoff: 1m, 5m, 30m,
/// 2h, 12h, 24h, 24h (capped). The worker stops re-trying after ~10 attempts at the call site.
pub(crate) async fn mark_delivery_failed<'e, E: PgExecutor<'e>>(
    ex: E,
    delivery_id: Uuid,
    attempts: i32,
    error: &str,
    now: DateTime<Utc>,
) -> Result<u64, sqlx::Error> {
    let backoff_secs = match attempts {
        0..=1 => 60,
        2 => 300,
        3 => 1_800,
        4 => 7_200,
        5 => 43_200,
        _ => 86_400,
    };
    let next = now + chrono::Duration::seconds(backoff_secs);
    let r = sqlx::query!(
        r#"
        UPDATE federation_delivery
           SET next_attempt_at = $2,
               last_error      = $3
         WHERE id = $1
        "#,
        delivery_id,
        next,
        error,
    )
    .execute(ex)
    .await?;
    Ok(r.rows_affected())
}

/// Remove an inbound follow row (called when the remote sends `Undo(Follow)`).
pub(crate) async fn delete_inbound_follow<'e, E: PgExecutor<'e>>(
    ex: E,
    citizen_id: Uuid,
    remote_actor_url: &str,
) -> Result<u64, sqlx::Error> {
    let r = sqlx::query!(
        r#"
        DELETE FROM federation_follow
         WHERE citizen_id = $1 AND direction = 'inbound' AND remote_actor_url = $2
        "#,
        citizen_id,
        remote_actor_url,
    )
    .execute(ex)
    .await?;
    Ok(r.rows_affected())
}

/// Persist a fresh OUTBOUND follow (we follow someone remote). Returns the surrogate id so the
/// caller can use it as the activity's `id` URL (we publish `<actor>/activities/<uuid>`). `ON
/// CONFLICT DO UPDATE` makes re-clicking "Seguir" on the same remote actor a refresh, not an
/// error. `accepted_at` stays NULL until the remote Accept comes back to our inbox.
pub(crate) async fn upsert_outbound_follow<'e, E: PgExecutor<'e>>(
    ex: E,
    id: Uuid,
    citizen_id: Uuid,
    remote_actor_url: &str,
    remote_inbox_url: &str,
    follow_activity_id: &str,
    now: DateTime<Utc>,
) -> Result<Uuid, sqlx::Error> {
    let row = sqlx::query!(
        r#"
        INSERT INTO federation_follow
            (id, citizen_id, direction, remote_actor_url, remote_inbox_url,
             follow_activity_id, accepted_at, created_at)
        VALUES ($1, $2, 'outbound', $3, $4, $5, NULL, $6)
        ON CONFLICT (citizen_id, direction, remote_actor_url) DO UPDATE
            SET remote_inbox_url   = EXCLUDED.remote_inbox_url,
                follow_activity_id = EXCLUDED.follow_activity_id
        RETURNING id
        "#,
        id,
        citizen_id,
        remote_actor_url,
        remote_inbox_url,
        follow_activity_id,
        now,
    )
    .fetch_one(ex)
    .await?;
    Ok(row.id)
}

/// Mark an outbound follow as accepted by the remote. Looked up by (citizen, remote_actor_url)
/// — the natural identity. Returns the affected row count so the inbox handler can distinguish
/// "Accept of something we sent" from "Accept of something we never sent" (idempotent: 0 = no
/// pending follow matches, harmless to no-op).
pub(crate) async fn mark_outbound_follow_accepted<'e, E: PgExecutor<'e>>(
    ex: E,
    citizen_id: Uuid,
    remote_actor_url: &str,
    now: DateTime<Utc>,
) -> Result<u64, sqlx::Error> {
    let r = sqlx::query!(
        r#"
        UPDATE federation_follow
           SET accepted_at = $3
         WHERE citizen_id = $1
           AND direction = 'outbound'
           AND remote_actor_url = $2
           AND accepted_at IS NULL
        "#,
        citizen_id,
        remote_actor_url,
        now,
    )
    .execute(ex)
    .await?;
    Ok(r.rows_affected())
}

/// Page of ACK'd outbound follows ("who I follow"). Drives the `/actors/{handle}/following`
/// OrderedCollection.
pub(crate) async fn list_outbound_following<'e, E: PgExecutor<'e>>(
    ex: E,
    citizen_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query_scalar!(
        r#"
        SELECT remote_actor_url
          FROM federation_follow
         WHERE citizen_id = $1
           AND direction = 'outbound'
           AND accepted_at IS NOT NULL
         ORDER BY created_at DESC
         LIMIT $2 OFFSET $3
        "#,
        citizen_id,
        limit,
        offset,
    )
    .fetch_all(ex)
    .await?;
    Ok(rows)
}

pub(crate) async fn count_outbound_following<'e, E: PgExecutor<'e>>(
    ex: E,
    citizen_id: Uuid,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query!(
        r#"
        SELECT count(*) AS "n!"
          FROM federation_follow
         WHERE citizen_id = $1
           AND direction = 'outbound'
           AND accepted_at IS NOT NULL
        "#,
        citizen_id,
    )
    .fetch_one(ex)
    .await?;
    Ok(row.n)
}

/// Total count of ACK'd inbound followers (used in the OrderedCollection's `totalItems`).
pub(crate) async fn count_inbound_followers<'e, E: PgExecutor<'e>>(
    ex: E,
    citizen_id: Uuid,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query!(
        r#"
        SELECT count(*) AS "n!"
          FROM federation_follow
         WHERE citizen_id = $1
           AND direction = 'inbound'
           AND accepted_at IS NOT NULL
        "#,
        citizen_id,
    )
    .fetch_one(ex)
    .await?;
    Ok(row.n)
}

/// Read the public PEM of a citizen's federation actor key, when it exists. The PRIVATE PEM is
/// deliberately NOT returned by this query — it is a credential and the request hot path that
/// renders the Actor document never needs it.
pub(crate) async fn find_actor_public_key<'e, E: PgExecutor<'e>>(
    ex: E,
    citizen_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query!(
        "SELECT public_pem FROM citizen_actor_key WHERE citizen_id = $1",
        citizen_id,
    )
    .fetch_optional(ex)
    .await?;
    Ok(row.map(|r| r.public_pem))
}

/// Insert a freshly-generated keypair (private+public PEM) for a citizen. Idempotent at the
/// schema level via the primary key; the service uses `ON CONFLICT DO NOTHING` so a concurrent
/// double-flip on `is_public` never races.
pub(crate) async fn insert_actor_keypair<'e, E: PgExecutor<'e>>(
    ex: E,
    citizen_id: Uuid,
    private_pem: &str,
    public_pem: &str,
    now: DateTime<Utc>,
) -> Result<u64, sqlx::Error> {
    let r = sqlx::query!(
        r#"
        INSERT INTO citizen_actor_key (citizen_id, private_pem, public_pem, created_at)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (citizen_id) DO NOTHING
        "#,
        citizen_id,
        private_pem,
        public_pem,
        now,
    )
    .execute(ex)
    .await?;
    Ok(r.rows_affected())
}

/// Resolve a public citizen by user-chosen handle (NOT the `public_handle` opaque id). Returns
/// `None` if the handle is unknown or the citizen has not opted into a public profile — the
/// federation surface is gated by `is_public = true` (ADR-0010 / LGPD).
pub(crate) async fn find_public_citizen_by_handle<'e, E: PgExecutor<'e>>(
    ex: E,
    org_id: Uuid,
    handle: &str,
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
               titulo_status,
               created_at
          FROM citizen
         WHERE org_id = $1 AND handle = $2 AND is_public = true
        "#,
        org_id,
        handle,
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

// -----------------------------------------------------------------------------
// Pending signup (migration 0106) — verificação de e-mail antes de criar conta
// -----------------------------------------------------------------------------

/// A pending signup redimível pelo token. Traz de volta tudo que o request
/// gravou pra que o confirm materialize citizen+credential+session numa única
/// tx sem re-perguntar nada ao usuário.
#[derive(Debug, Clone)]
pub(crate) struct PendingSignupRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub email: String,
    pub password_hash: String,
    pub cpf: String,
    pub role: String,
    pub mandate_id: Option<Uuid>,
}

/// Insere um pending_signup. Caller pré-computou o SHA-256 do token (só o
/// hash entra no banco) e já normalizou email/cpf/role.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn pending_signup_insert<'e, E: PgExecutor<'e>>(
    ex: E,
    id: Uuid,
    org_id: Uuid,
    email: &str,
    password_hash: &str,
    cpf: &str,
    role: &str,
    mandate_id: Option<Uuid>,
    token_hash: &[u8],
    expires_at: DateTime<Utc>,
    request_ip: Option<&str>,
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO auth_pending_signup
            (id, org_id, email, password_hash, cpf, role, mandate_id,
             token_hash, expires_at, used_at, request_ip, created_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NULL, $10, $11)
        "#,
        id,
        org_id,
        email,
        password_hash,
        cpf,
        role,
        mandate_id,
        token_hash,
        expires_at,
        request_ip,
        now,
    )
    .execute(ex)
    .await?;
    Ok(())
}

/// Marca como usada qualquer pending live pra `(org_id, email)`. Chamada
/// antes de inserir uma nova — o link mais recente sempre vence. Same UX que
/// o password_reset.
pub(crate) async fn pending_signup_invalidate_live_for_email<'e, E: PgExecutor<'e>>(
    ex: E,
    org_id: Uuid,
    email: &str,
    now: DateTime<Utc>,
) -> Result<u64, sqlx::Error> {
    let r = sqlx::query!(
        r#"
        UPDATE auth_pending_signup
           SET used_at = $3
         WHERE org_id = $1 AND email = $2 AND used_at IS NULL
        "#,
        org_id,
        email,
        now,
    )
    .execute(ex)
    .await?;
    Ok(r.rows_affected())
}

/// Look up pending redimível por token_hash + guarda de expiração. Retorna
/// `None` pra token desconhecido / expirado / já usado (o confirm nunca diz
/// ao chamador qual caso ocorreu).
pub(crate) async fn pending_signup_find_live<'e, E: PgExecutor<'e>>(
    ex: E,
    token_hash: &[u8],
    now: DateTime<Utc>,
) -> Result<Option<PendingSignupRow>, sqlx::Error> {
    let row = sqlx::query_as!(
        PendingSignupRow,
        r#"
        SELECT id, org_id, email, password_hash, cpf, role, mandate_id
          FROM auth_pending_signup
         WHERE token_hash = $1 AND used_at IS NULL AND expires_at > $2
        "#,
        token_hash,
        now,
    )
    .fetch_optional(ex)
    .await?;
    Ok(row)
}

/// Conta pending_signups criadas por um `request_ip` desde `since`. Usada
/// pelo rate-limit do cadastro: bots com CPFs válidos poderiam floodar
/// pending_signups; limitamos 3/hora por IP como defesa em profundidade
/// (o SMTP relay já rejeitaria envios em massa, mas melhor não chegar lá).
/// `request_ip = NULL` (X-Forwarded-For ausente) escapa por design — nunca
/// contamos o mesmo bucket "sem-IP".
pub(crate) async fn pending_signup_count_by_ip_since<'e, E: PgExecutor<'e>>(
    ex: E,
    request_ip: &str,
    since: DateTime<Utc>,
) -> Result<i64, sqlx::Error> {
    let count = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "count!" FROM auth_pending_signup
           WHERE request_ip = $1 AND created_at >= $2"#,
        request_ip,
        since,
    )
    .fetch_one(ex)
    .await?;
    Ok(count)
}

/// Acha a pending live mais recente por `(org_id, email)`, se houver. Usada
/// pelo resend endpoint: reaproveita password_hash+cpf+role+mandate_id do
/// pending que ainda está vivo (senão o usuário teria que digitar tudo de
/// novo). Mesma UX que password_reset com "novo link mata o anterior".
pub(crate) async fn pending_signup_find_live_for_email<'e, E: PgExecutor<'e>>(
    ex: E,
    org_id: Uuid,
    email: &str,
    now: DateTime<Utc>,
) -> Result<Option<PendingSignupRow>, sqlx::Error> {
    let row = sqlx::query_as!(
        PendingSignupRow,
        r#"
        SELECT id, org_id, email, password_hash, cpf, role, mandate_id
          FROM auth_pending_signup
         WHERE org_id = $1 AND email = $2
           AND used_at IS NULL AND expires_at > $3
         ORDER BY created_at DESC
         LIMIT 1
        "#,
        org_id,
        email,
        now,
    )
    .fetch_optional(ex)
    .await?;
    Ok(row)
}

/// Deleta pendings antigos vencidos há mais de `cutoff_days` — cleanup
/// worker (P3.3). Idempotente.
pub(crate) async fn pending_signup_cleanup_expired<'e, E: PgExecutor<'e>>(
    ex: E,
    cutoff: DateTime<Utc>,
) -> Result<u64, sqlx::Error> {
    let r = sqlx::query!(
        "DELETE FROM auth_pending_signup WHERE expires_at < $1",
        cutoff,
    )
    .execute(ex)
    .await?;
    Ok(r.rows_affected())
}

/// Registra uma tentativa de login (rate limit + auditoria, P5.1). Insert
/// simples — a política de bloqueio é do serviço.
pub(crate) async fn login_attempt_record<'e, E: PgExecutor<'e>>(
    ex: E,
    request_ip: &str,
    outcome: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "INSERT INTO auth_login_attempt (request_ip, outcome) VALUES ($1, $2)",
        request_ip,
        outcome,
    )
    .execute(ex)
    .await?;
    Ok(())
}

/// Conta tentativas de login de um IP desde `since`. `sucesso + falha` — a
/// política é limitar QUALQUER volume anormal, não só falhas.
pub(crate) async fn login_attempt_count_by_ip_since<'e, E: PgExecutor<'e>>(
    ex: E,
    request_ip: &str,
    since: DateTime<Utc>,
) -> Result<i64, sqlx::Error> {
    let count = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "count!" FROM auth_login_attempt
           WHERE request_ip = $1 AND at >= $2"#,
        request_ip,
        since,
    )
    .fetch_one(ex)
    .await?;
    Ok(count)
}

/// Limpa tentativas antigas — invocada pelo mesmo worker do
/// pending_signup_cleanup.
pub(crate) async fn login_attempt_cleanup<'e, E: PgExecutor<'e>>(
    ex: E,
    cutoff: DateTime<Utc>,
) -> Result<u64, sqlx::Error> {
    let r = sqlx::query!("DELETE FROM auth_login_attempt WHERE at < $1", cutoff,)
        .execute(ex)
        .await?;
    Ok(r.rows_affected())
}

/// Marca a pending como usada (single-use). Chamada dentro da tx do confirm
/// junto com o insert do citizen/credential; se a tx roll-back, a linha
/// continua redimível.
pub(crate) async fn pending_signup_mark_used<'e, E: PgExecutor<'e>>(
    ex: E,
    id: Uuid,
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE auth_pending_signup SET used_at = $2 WHERE id = $1",
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
    let r = sqlx::query!("DELETE FROM auth_session WHERE citizen_id = $1", citizen_id,)
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
                  titulo_status,
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

//! # Database access for public petitions (runtime `sqlx`, explicit SQL, no `SELECT *`).

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::sso_oidc::{create_account, NewAccount};

/// A petition as the API shows it.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub(crate) struct PetitionRow {
    pub id: Uuid,
    pub title: String,
    pub body: String,
    pub threshold: i32,
    pub signature_count: i64,
    pub reached_at: Option<DateTime<Utc>>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    pub author_id: Option<Uuid>,
    /// An admin set the current status; the author cannot change it.
    pub moderated: bool,
    pub author_name: Option<String>,
}

const PETITION_SELECT: &str = r"
    SELECT i.id, i.title, i.body, i.threshold, i.signature_count, i.reached_at, i.status,
           i.created_at, i.closed_at, i.author_id, i.moderated,
           COALESCE(c.display_name, c.handle) AS author_name
      FROM initiative i
      LEFT JOIN citizen c ON c.id = i.author_id";

/// Petitions of an org. `include_hidden` is for admins.
pub(crate) async fn list(
    db: &PgPool,
    org: Uuid,
    status: Option<&str>,
    include_hidden: bool,
    limit: i64,
) -> Result<Vec<PetitionRow>, sqlx::Error> {
    sqlx::query_as(&format!(
        "{PETITION_SELECT}
          WHERE i.org_id = $1 AND i.author_id IS NOT NULL
            AND ($2::text IS NULL OR i.status = $2)
            AND (i.status <> 'hidden' OR $3)
          ORDER BY i.created_at DESC
          LIMIT $4"
    ))
    .bind(org)
    .bind(status)
    .bind(include_hidden)
    .bind(limit)
    .fetch_all(db)
    .await
}

/// One petition, hidden ones only for admins.
pub(crate) async fn get(
    db: &PgPool,
    org: Uuid,
    id: Uuid,
    include_hidden: bool,
) -> Result<Option<PetitionRow>, sqlx::Error> {
    sqlx::query_as(&format!(
        "{PETITION_SELECT}
          WHERE i.org_id = $1 AND i.id = $2 AND i.author_id IS NOT NULL
            AND (i.status <> 'hidden' OR $3)"
    ))
    .bind(org)
    .bind(id)
    .bind(include_hidden)
    .fetch_optional(db)
    .await
}

pub(crate) async fn create(
    db: &PgPool,
    org: Uuid,
    author: Uuid,
    title: &str,
    body: &str,
    threshold: i32,
) -> Result<Uuid, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO initiative
             (id, org_id, title, body, threshold, signature_count, reached_at, created_at,
              author_id, status)
         VALUES ($1, $2, $3, $4, $5, 0, NULL, now(), $6, 'open')
         RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(org)
    .bind(title)
    .bind(body)
    .bind(threshold)
    .bind(author)
    .fetch_one(db)
    .await
}

/// Change the status. `by_admin` marks the change as moderation: the author can no
/// longer undo it (for example, reopen what an admin closed).
pub(crate) async fn set_status(
    db: &PgPool,
    org: Uuid,
    id: Uuid,
    status: &str,
    by_admin: bool,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE initiative
            SET status = $3,
                moderated = CASE WHEN $4 THEN $3 <> 'open' ELSE moderated END,
                closed_at = CASE WHEN $3 = 'open' THEN NULL ELSE COALESCE(closed_at, now()) END
          WHERE org_id = $1 AND id = $2 AND author_id IS NOT NULL",
    )
    .bind(org)
    .bind(id)
    .bind(status)
    .bind(by_admin)
    .execute(db)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// A signer shown on the petition page (only those who asked to be shown).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub(crate) struct PublicSigner {
    pub name: String,
    pub city: Option<String>,
    pub signed_at: DateTime<Utc>,
}

pub(crate) async fn recent_signers(
    db: &PgPool,
    id: Uuid,
    limit: i64,
) -> Result<Vec<PublicSigner>, sqlx::Error> {
    sqlx::query_as(
        "SELECT public_name AS name, city, created_at AS signed_at
           FROM initiative_signature
          WHERE initiative_id = $1 AND public_name IS NOT NULL
          ORDER BY created_at DESC
          LIMIT $2",
    )
    .bind(id)
    .bind(limit)
    .fetch_all(db)
    .await
}

pub(crate) async fn has_signed(db: &PgPool, id: Uuid, citizen: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM initiative_signature WHERE initiative_id = $1 AND citizen_id = $2)",
    )
    .bind(id)
    .bind(citizen)
    .fetch_one(db)
    .await
}

/// The tally after a signature.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Tally {
    pub petition_id: Uuid,
    pub signature_count: i64,
    pub threshold: i32,
    pub reached: bool,
    pub newly_signed: bool,
}

/// Why a signature was not recorded.
#[derive(Debug)]
pub(crate) enum SignError {
    NotFound,
    Closed,
    Blocked,
    Db(sqlx::Error),
}

impl From<sqlx::Error> for SignError {
    fn from(e: sqlx::Error) -> Self {
        Self::Db(e)
    }
}

/// Record one citizen's signature in `tx`. Idempotent; the tally and the one-shot
/// `reached_at` marker change in the same transaction.
pub(crate) async fn sign(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    id: Uuid,
    citizen: Uuid,
    public_name: Option<&str>,
    city: Option<&str>,
) -> Result<Tally, SignError> {
    let status: Option<String> = sqlx::query_scalar(
        "SELECT status FROM initiative WHERE org_id = $1 AND id = $2 AND author_id IS NOT NULL FOR UPDATE",
    )
    .bind(org)
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?;
    match status.as_deref() {
        None | Some("hidden") => return Err(SignError::NotFound),
        Some("open") => {}
        Some(_) => return Err(SignError::Closed),
    }
    let blocked: bool = sqlx::query_scalar(
        "SELECT suspended_at IS NOT NULL OR deleted_at IS NOT NULL FROM citizen WHERE id = $1",
    )
    .bind(citizen)
    .fetch_one(&mut **tx)
    .await?;
    if blocked {
        return Err(SignError::Blocked);
    }
    let inserted: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO initiative_signature (id, initiative_id, citizen_id, created_at, public_name, city)
         VALUES ($1, $2, $3, now(), $4, $5)
         ON CONFLICT (initiative_id, citizen_id) DO NOTHING
         RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(id)
    .bind(citizen)
    .bind(public_name)
    .bind(city)
    .fetch_optional(&mut **tx)
    .await?;
    let (count, threshold, reached): (i64, i32, Option<DateTime<Utc>>) = sqlx::query_as(
        "UPDATE initiative
            SET signature_count = signature_count + $2,
                reached_at = CASE WHEN reached_at IS NULL AND signature_count + $2 >= threshold
                                  THEN now() ELSE reached_at END
          WHERE id = $1
          RETURNING signature_count, threshold, reached_at",
    )
    .bind(id)
    .bind(i64::from(inserted.is_some()))
    .fetch_one(&mut **tx)
    .await?;
    Ok(Tally {
        petition_id: id,
        signature_count: count,
        threshold,
        reached: reached.is_some(),
        newly_signed: inserted.is_some(),
    })
}

/// Find the citizen for a confirmed signer, or create one.
///
/// Order: the provider subject, then an existing credential with the same verified
/// address, then a new account. This is the same rule the SSO login uses, so a later
/// login lands on the same account.
pub(crate) async fn ensure_citizen(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    subject: Option<&str>,
    email: &str,
    name: &str,
    handle: Option<&str>,
) -> Result<Uuid, sqlx::Error> {
    if let Some(sub) = subject {
        let found: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM citizen WHERE org_id = $1 AND oidc_subject = $2")
                .bind(org)
                .bind(sub)
                .fetch_optional(&mut **tx)
                .await?;
        if let Some(id) = found {
            return Ok(id);
        }
    }
    let by_email: Option<Uuid> = sqlx::query_scalar(
        // Only an account that already PROVED this address may be reused. A credential
        // typed into /auth/register but never confirmed (level 'anonymous') could belong
        // to someone squatting the address; linking it would hand them the signer's
        // identity-provider subject.
        "SELECT a.citizen_id FROM auth_credential a JOIN citizen c ON c.id = a.citizen_id
          WHERE a.org_id = $1 AND a.email = $2 AND c.verification_level <> 'anonymous'
            AND c.deleted_at IS NULL",
    )
    .bind(org)
    .bind(email)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(id) = by_email {
        if let Some(sub) = subject {
            sqlx::query(
                "UPDATE citizen SET oidc_subject = $2 WHERE id = $1 AND oidc_subject IS NULL",
            )
            .bind(id)
            .bind(sub)
            .execute(&mut **tx)
            .await?;
        }
        return Ok(id);
    }
    let account = NewAccount {
        subject,
        name: Some(name),
        preferred_handle: handle,
        email: Some(email),
    };
    create_account(tx, org, &account).await
}

/// A pending signature as stored.
#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct Pending {
    pub id: Uuid,
    pub initiative_id: Uuid,
    pub email: String,
    pub name: String,
    pub city: Option<String>,
    pub show_name: bool,
    pub sso_user_id: Option<String>,
    pub sso_created: bool,
}

const PENDING_COLS: &str =
    "id, initiative_id, email, name, city, show_name, sso_user_id, sso_created";

/// The new or refreshed pending request.
pub(crate) struct PendingInput<'a> {
    pub id: Uuid,
    pub initiative_id: Uuid,
    pub email: &'a str,
    pub name: &'a str,
    pub city: Option<&'a str>,
    pub show_name: bool,
    pub token_hash: Option<&'a [u8]>,
    pub sso_user_id: Option<&'a str>,
    pub sso_created: bool,
    pub ttl_hours: i64,
}

/// Insert, or refresh the open request for the same address. Returns the row id in use.
pub(crate) async fn upsert_pending(db: &PgPool, p: &PendingInput<'_>) -> Result<Uuid, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO initiative_pending_signature
             (id, initiative_id, email, name, city, show_name, token_hash, sso_user_id,
              sso_created, created_at, last_sent_at, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now(), now(), now() + make_interval(hours => $10))
         ON CONFLICT (initiative_id, email) DO UPDATE
            SET name = EXCLUDED.name, city = EXCLUDED.city, show_name = EXCLUDED.show_name,
                token_hash = EXCLUDED.token_hash,
                sso_user_id = COALESCE(EXCLUDED.sso_user_id, initiative_pending_signature.sso_user_id),
                sso_created = initiative_pending_signature.sso_created OR EXCLUDED.sso_created,
                last_sent_at = now(), expires_at = EXCLUDED.expires_at
         RETURNING id",
    )
    .bind(p.id)
    .bind(p.initiative_id)
    .bind(p.email)
    .bind(p.name)
    .bind(p.city)
    .bind(p.show_name)
    .bind(p.token_hash)
    .bind(p.sso_user_id)
    .bind(p.sso_created)
    .bind(i32::try_from(p.ttl_hours).unwrap_or(72))
    .fetch_one(db)
    .await
}

/// The open request for an address, if any (used to reuse a provider user).
pub(crate) async fn pending_for_email(
    db: &PgPool,
    initiative: Uuid,
    email: &str,
) -> Result<Option<Pending>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {PENDING_COLS} FROM initiative_pending_signature
          WHERE initiative_id = $1 AND email = $2 AND expires_at > now()"
    ))
    .bind(initiative)
    .bind(email)
    .fetch_optional(db)
    .await
}

/// Lock the live request with this token hash for the rest of `tx`. A second,
/// concurrent confirmation waits here and then finds the row gone.
pub(crate) async fn claim_by_token(
    tx: &mut Transaction<'_, Postgres>,
    hash: &[u8],
) -> Result<Option<Pending>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {PENDING_COLS} FROM initiative_pending_signature
          WHERE token_hash = $1 AND expires_at > now()
          FOR UPDATE"
    ))
    .bind(hash)
    .fetch_optional(&mut **tx)
    .await
}

/// Same as [`claim_by_token`], by row id (sso mode).
pub(crate) async fn claim_by_id(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<Option<Pending>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {PENDING_COLS} FROM initiative_pending_signature
          WHERE id = $1 AND expires_at > now()
          FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(&mut **tx)
    .await
}

/// Serialize every account decision for one address until `tx` ends, so two
/// confirmations for different petitions never create two citizens.
pub(crate) async fn lock_address(
    tx: &mut Transaction<'_, Postgres>,
    email: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('petition-signer:' || $1, 0))")
        .bind(email)
        .execute(&mut **tx)
        .await
        .map(|_| ())
}

/// Seconds since the last e-mail to this address for this petition, if one went out.
pub(crate) async fn seconds_since_sent(
    db: &PgPool,
    initiative: Uuid,
    email: &str,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT extract(epoch FROM now() - last_sent_at)::bigint
           FROM initiative_pending_signature
          WHERE initiative_id = $1 AND email = $2 AND expires_at > now()",
    )
    .bind(initiative)
    .bind(email)
    .fetch_optional(db)
    .await
}

/// Confirmation e-mails sent in the last hour, across the whole instance.
pub(crate) async fn sent_last_hour(db: &PgPool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT count(*) FROM initiative_pending_signature
          WHERE last_sent_at > now() - interval '1 hour'",
    )
    .fetch_one(db)
    .await
}

pub(crate) async fn delete_pending(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM initiative_pending_signature WHERE id = $1")
        .bind(id)
        .execute(&mut **tx)
        .await
        .map(|_| ())
}

/// Remove up to `limit` expired requests. Returns the provider users this server
/// created for them, so the caller can remove those too.
pub(crate) async fn take_expired(db: &PgPool, limit: i64) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "DELETE FROM initiative_pending_signature
          WHERE id IN (SELECT id FROM initiative_pending_signature
                        WHERE expires_at <= now() ORDER BY expires_at LIMIT $1)
          RETURNING CASE WHEN sso_created THEN sso_user_id END",
    )
    .bind(limit)
    .fetch_all(db)
    .await
    .map(|ids: Vec<Option<String>>| ids.into_iter().flatten().collect())
}

/// Record a used proof-of-work. `false` means it was already used (a replay).
pub(crate) async fn spend_altcha(
    db: &PgPool,
    signature: &str,
    expires_unix: i64,
) -> Result<bool, sqlx::Error> {
    sqlx::query("DELETE FROM altcha_spent WHERE expires_at < now()")
        .execute(db)
        .await?;
    let done = sqlx::query(
        "INSERT INTO altcha_spent (challenge, expires_at) VALUES ($1, $2)
         ON CONFLICT DO NOTHING",
    )
    .bind(signature)
    .bind(DateTime::<Utc>::from_timestamp(expires_unix, 0))
    .execute(db)
    .await?;
    Ok(done.rows_affected() == 1)
}

pub(crate) async fn is_org_admin(
    db: &PgPool,
    org: Uuid,
    citizen: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM admin_role_binding
          WHERE org_id = $1 AND citizen_id = $2 AND role IN ('owner', 'admin'))",
    )
    .bind(org)
    .bind(citizen)
    .fetch_one(db)
    .await
}

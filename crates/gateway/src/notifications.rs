//! # User-facing notifications (`user_notification`, migration 0406).
//!
//! Populated at every inbox event that affects a local citizen: someone liked
//! their post (`favourite`), boosted it (`reblog`), replied to it (`reply`),
//! mentioned them (`mention`), or followed them (`follow`).
//!
//! All queries are runtime-unchecked `sqlx::query*` — see `federation_feed.rs`
//! for the policy note.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

/// One item of the `/api/v1/me/notifications` feed. `read` is derived from
/// `read_at` on the way out so the front-end doesn't parse the timestamp.
#[derive(Debug, Serialize)]
pub struct NotificationDto {
    pub id: Uuid,
    /// One of `mention` | `reply` | `favourite` | `reblog` | `follow`.
    pub kind: String,
    pub source_actor_url: Option<String>,
    /// Best display handle: `alice` (local) or `alice@remote.tld` (remote).
    pub source_handle: String,
    pub source_display_name: Option<String>,
    pub source_avatar_url: Option<String>,
    /// Related Note URI (NULL for `follow`).
    pub object_uri: Option<String>,
    /// Plain-text preview of the note (max 240 chars).
    pub object_preview: Option<String>,
    pub created_at: DateTime<Utc>,
    pub read: bool,
}

#[derive(Debug, sqlx::FromRow)]
struct NotificationRow {
    id: Uuid,
    kind: String,
    source_actor_url: Option<String>,
    source_handle: String,
    source_display_name: Option<String>,
    source_avatar_url: Option<String>,
    object_uri: Option<String>,
    object_preview: Option<String>,
    created_at: DateTime<Utc>,
    read_at: Option<DateTime<Utc>>,
}

impl From<NotificationRow> for NotificationDto {
    fn from(r: NotificationRow) -> Self {
        Self {
            id: r.id,
            kind: r.kind,
            source_actor_url: r.source_actor_url,
            source_handle: r.source_handle,
            source_display_name: r.source_display_name,
            source_avatar_url: r.source_avatar_url,
            object_uri: r.object_uri,
            object_preview: r.object_preview,
            created_at: r.created_at,
            read: r.read_at.is_some(),
        }
    }
}

/// Bag of fields passed to `insert` — beats a 9-arg call at every callsite.
#[derive(Debug, Clone)]
pub struct NewNotification<'a> {
    pub citizen_id: Uuid,
    pub kind: &'a str,
    pub source_actor_url: Option<&'a str>,
    pub source_handle: &'a str,
    pub source_display_name: Option<&'a str>,
    pub source_avatar_url: Option<&'a str>,
    pub object_uri: Option<&'a str>,
    pub object_preview: Option<&'a str>,
}

/// Idempotent insert. A re-delivered inbox event with the same
/// (recipient, kind, source_actor_url, object_uri) tuple is a no-op.
pub async fn insert(db: &PgPool, n: NewNotification<'_>) -> Result<(), sqlx::Error> {
    let now = Utc::now();
    // Truncate preview at 240 chars (byte-safe via chars().take()).
    let preview_owned: Option<String> = n
        .object_preview
        .map(|s| s.chars().take(240).collect::<String>());
    sqlx::query(
        r"
        INSERT INTO user_notification
            (id, citizen_id, kind, source_actor_url, source_handle,
             source_display_name, source_avatar_url, object_uri,
             object_preview, created_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        ON CONFLICT (citizen_id, kind, source_actor_url, object_uri) DO NOTHING
        ",
    )
    .bind(Uuid::now_v7())
    .bind(n.citizen_id)
    .bind(n.kind)
    .bind(n.source_actor_url)
    .bind(n.source_handle)
    .bind(n.source_display_name)
    .bind(n.source_avatar_url)
    .bind(n.object_uri)
    .bind(preview_owned.as_deref())
    .bind(now)
    .execute(db)
    .await?;
    Ok(())
}

/// Feed for the citizen's `/notificacoes` page. Newest-first, paginated,
/// unread mixed with read.
pub async fn list_for_citizen(
    db: &PgPool,
    citizen_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<Vec<NotificationDto>, sqlx::Error> {
    let rows = sqlx::query_as::<_, NotificationRow>(
        r"
        SELECT id, kind, source_actor_url, source_handle,
               source_display_name, source_avatar_url,
               object_uri, object_preview, created_at, read_at
          FROM user_notification
         WHERE citizen_id = $1
         ORDER BY created_at DESC
         LIMIT $2 OFFSET $3
        ",
    )
    .bind(citizen_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(NotificationDto::from).collect())
}

/// Unread count (drives the bell badge in the top nav).
pub async fn unread_count(db: &PgPool, citizen_id: Uuid) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        r"SELECT count(*) FROM user_notification
           WHERE citizen_id = $1 AND read_at IS NULL",
    )
    .bind(citizen_id)
    .fetch_one(db)
    .await
}

/// Stamp every unread row as read for this citizen. Returns rows updated.
pub async fn mark_all_read(db: &PgPool, citizen_id: Uuid) -> Result<u64, sqlx::Error> {
    let r = sqlx::query(
        r"UPDATE user_notification SET read_at = now()
           WHERE citizen_id = $1 AND read_at IS NULL",
    )
    .bind(citizen_id)
    .execute(db)
    .await?;
    Ok(r.rows_affected())
}

/// Resolve a local Actor URL (`{public_origin}/actors/{handle}`) to the owning
/// `citizen.id`. Returns `None` for remote or unknown URLs.
pub async fn find_local_citizen_by_actor_url(
    db: &PgPool,
    actor_url: &str,
    public_origin: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    // Extract the handle from the URL suffix if it matches our origin.
    let prefix = format!("{}/actors/", public_origin.trim_end_matches('/'));
    let Some(handle) = actor_url.strip_prefix(&prefix) else {
        return Ok(None);
    };
    // Strip any trailing collection paths (`/followers`, `/outbox`, …).
    let handle = handle.split('/').next().unwrap_or(handle);
    if handle.is_empty() {
        return Ok(None);
    }
    sqlx::query_scalar::<_, Uuid>(
        r"SELECT id FROM citizen WHERE handle = $1 AND is_public = true LIMIT 1",
    )
    .bind(handle)
    .fetch_optional(db)
    .await
}

/// Given the URI of a Note in our outbox, return the owning `citizen.id`.
pub async fn find_owner_of_object(
    db: &PgPool,
    object_uri: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar::<_, Uuid>(
        r"
        SELECT citizen_id
          FROM federation_outbox_entry
         WHERE activity_id = $1
            OR payload->'object'->>'id' = $1
         LIMIT 1
        ",
    )
    .bind(object_uri)
    .fetch_optional(db)
    .await
}

/// Small helper: strip HTML tags from a note body for the preview snippet.
/// Not a full HTML parser — deliberate: we just want plain text. Runs on a
/// short prefix so this is O(n) with a tiny constant.
#[must_use]
pub fn preview_from_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut inside = false;
    for c in html.chars() {
        match c {
            '<' => inside = true,
            '>' => inside = false,
            _ if !inside => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

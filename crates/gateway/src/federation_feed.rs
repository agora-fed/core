//! # Federated feed + reactions — gateway-owned persistence (ADR-0010 W2.6).
//!
//! Every SQL statement behind the federated timeline (`GET /api/v1/me/feed`) and the reaction
//! toggles (`POST /api/v1/me/like`, `POST /api/v1/me/boost`) plus the inbox-side upserts for
//! remote `Create(Note)` / `Like` / `Announce` activities.
//!
//! All queries are RUNTIME (unchecked) `sqlx::query*` calls — never the compile-checked macros —
//! so the committed `.sqlx/` offline cache does not need regenerating on a DB-less build host
//! (same policy as `parlamentar_activity.rs`).
//!
//! Tables (migration `0403_federation_feed_reactions.sql`):
//! * `federation_timeline_entry`  — remote Notes, one row per `object_uri`.
//! * `federation_reaction`        — local citizen Like/Boost toggles.
//! * `federation_remote_reaction` — remote Like/Announce over OUR objects.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

/// Max stored size for a remote Note's content (bytes, pre-sanitize). Mirrors the DB CHECK.
pub const CONTENT_MAX_BYTES: usize = 64 * 1024;

/// One item of the merged federated feed — EXACTLY the wire shape the front was coded against.
#[derive(Debug, Serialize)]
pub struct FeedItemDto {
    /// The Note object's URI (local: `<actor>/objects/<uuid>`; remote: whatever the wire gave).
    pub object_uri: String,
    /// `@fulana` for local authors; `user@remote.tld` for remote ones.
    pub author_handle: String,
    pub author_display_name: Option<String>,
    pub author_avatar_url: Option<String>,
    pub content_html: String,
    pub published_at: DateTime<Utc>,
    pub is_remote: bool,
    /// Local + remote reactions combined.
    pub like_count: i64,
    pub boost_count: i64,
    pub liked_by_me: bool,
    pub boosted_by_me: bool,
    /// 0.18.0: parent Note object URI (for threaded replies). NULL = top-level.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_reply_to_uri: Option<String>,
    /// 0.18.0: Mastodon-style sensitive flag.
    #[serde(default)]
    pub sensitive: bool,
    /// 0.18.0: content-warning header.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spoiler_text: Option<String>,
    /// 0.18.0-gamma: media attachments (empty when the note carries no media).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<crate::note_media::MediaDto>,
    /// 0.18.0-rc1: when the author edited the Note. NULL for never-edited.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<DateTime<Utc>>,
    /// 0.18.0-rc1: poll DTO when the Note carries a Question, None otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poll: Option<crate::polls::PollDto>,
}

#[derive(Debug, sqlx::FromRow)]
struct FeedRow {
    object_uri: String,
    author_handle: String,
    author_display_name: Option<String>,
    author_avatar_url: Option<String>,
    content_html: String,
    published_at: DateTime<Utc>,
    is_remote: bool,
    like_count: i64,
    boost_count: i64,
    liked_by_me: bool,
    boosted_by_me: bool,
    in_reply_to_uri: Option<String>,
    sensitive: bool,
    spoiler_text: Option<String>,
    edited_at: Option<DateTime<Utc>>,
}

impl From<FeedRow> for FeedItemDto {
    fn from(r: FeedRow) -> Self {
        Self {
            object_uri: r.object_uri,
            author_handle: r.author_handle,
            author_display_name: r.author_display_name,
            author_avatar_url: r.author_avatar_url,
            content_html: r.content_html,
            published_at: r.published_at,
            is_remote: r.is_remote,
            like_count: r.like_count,
            boost_count: r.boost_count,
            liked_by_me: r.liked_by_me,
            boosted_by_me: r.boosted_by_me,
            in_reply_to_uri: r.in_reply_to_uri,
            sensitive: r.sensitive,
            spoiler_text: r.spoiler_text,
            edited_at: r.edited_at,
            attachments: Vec::new(),
            poll: None,
        }
    }
}

/// Batch-attach media to a set of DTOs. Handler calls this AFTER `list_feed`
/// so the SQL stays simple and media joins happen in one round-trip.
pub async fn enrich_with_media(db: &PgPool, items: &mut [FeedItemDto], media_base_url: &str) {
    if items.is_empty() {
        return;
    }
    let uris: Vec<String> = items.iter().map(|i| i.object_uri.clone()).collect();
    if let Ok(map) = crate::note_media::list_for_notes(db, &uris, media_base_url).await {
        for it in items.iter_mut() {
            if let Some(v) = map.get(&it.object_uri) {
                it.attachments = v.clone();
            }
        }
    }
}

/// Batch-attach poll DTOs. `viewer_actor_url` scopes `voted_option_ids` per
/// caller so the client renders "you already voted" markers.
pub async fn enrich_with_polls(
    db: &PgPool,
    items: &mut [FeedItemDto],
    viewer_actor_url: Option<&str>,
) {
    if items.is_empty() {
        return;
    }
    let uris: Vec<String> = items.iter().map(|i| i.object_uri.clone()).collect();
    if let Ok(map) = crate::polls::list_for_notes(db, &uris, viewer_actor_url).await {
        for it in items.iter_mut() {
            if let Some(p) = map.get(&it.object_uri) {
                it.poll = Some(p.clone());
            }
        }
    }
}

/// Assemble the citizen's merged feed:
/// (a) local Notes authored by the citizen or by LOCAL citizens they follow (an outbound follow
///     whose `remote_actor_url` is `{public_origin}/actors/{handle}` — that is how a local→local
///     follow lands, since `/me/follow` webfinger-resolves our own instance like any other), and
/// (b) remote Notes (`federation_timeline_entry`) from actors they follow,
/// ordered by `published_at DESC`. Counts merge local + remote reactions.
///
/// 0.20.0-beta fase 2A: rows are excluded when the caller
/// * mutes the author (`actor_mute`),
/// * blocks the author (`actor_block`), or
/// * has a live `content_filter` with context 'home' whose phrase is a
///   case-insensitive substring of the note content.
///
/// For LOCAL entries the author's actor_url is composed as
/// `{public_origin}/actors/{handle}`; for REMOTE entries it is
/// `federation_timeline_entry.actor_url` verbatim.
pub async fn list_feed(
    db: &PgPool,
    citizen_id: Uuid,
    public_origin: &str,
    media_base_url: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<FeedItemDto>, sqlx::Error> {
    let rows = sqlx::query_as::<_, FeedRow>(
        r"
        SELECT feed.object_uri,
               feed.author_handle,
               feed.author_display_name,
               feed.author_avatar_url,
               feed.content_html,
               feed.published_at,
               feed.is_remote,
               feed.in_reply_to_uri,
               feed.sensitive,
               feed.spoiler_text,
               feed.edited_at,
               (SELECT count(*) FROM federation_reaction r
                 WHERE r.object_uri = feed.object_uri AND r.kind = 'like')
             + (SELECT count(*) FROM federation_remote_reaction rr
                 WHERE rr.object_uri = feed.object_uri AND rr.kind = 'like')  AS like_count,
               (SELECT count(*) FROM federation_reaction r
                 WHERE r.object_uri = feed.object_uri AND r.kind = 'boost')
             + (SELECT count(*) FROM federation_remote_reaction rr
                 WHERE rr.object_uri = feed.object_uri AND rr.kind = 'boost') AS boost_count,
               EXISTS (SELECT 1 FROM federation_reaction r
                        WHERE r.citizen_id = $1 AND r.object_uri = feed.object_uri
                          AND r.kind = 'like')  AS liked_by_me,
               EXISTS (SELECT 1 FROM federation_reaction r
                        WHERE r.citizen_id = $1 AND r.object_uri = feed.object_uri
                          AND r.kind = 'boost') AS boosted_by_me
          FROM (
                -- DISTINCT ON: the same note may exist locally (outbox) AND ingested via
                -- federacao (loop no proprio dominio). Duplicata derruba o {#each}
                -- chaveado do front (each_key_duplicate) -- prefere a linha local.
                SELECT DISTINCT ON (u.object_uri) u.*
                  FROM (
                SELECT COALESCE(oe.payload->'object'->>'id', oe.activity_id) AS object_uri,
                       '@' || COALESCE(c.handle, 'u-' || replace(c.id::text, '-', ''))
                                                                             AS author_handle,
                       c.display_name                                        AS author_display_name,
                       CASE WHEN c.avatar_object_key IS NOT NULL AND $2 <> ''
                            THEN $2 || '/' || c.avatar_object_key END        AS author_avatar_url,
                       COALESCE(oe.payload->'object'->>'content', '')        AS content_html,
                       oe.created_at                                         AS published_at,
                       false                                                 AS is_remote,
                       oe.in_reply_to_uri                                    AS in_reply_to_uri,
                       oe.sensitive                                          AS sensitive,
                       oe.spoiler_text                                       AS spoiler_text,
                       oe.edited_at                                          AS edited_at,
                       CASE WHEN c.handle IS NOT NULL
                            THEN $3 || '/actors/' || c.handle END            AS author_actor_url
                  FROM federation_outbox_entry oe
                  JOIN citizen c ON c.id = oe.citizen_id
                 WHERE oe.kind = 'Create'
                   AND oe.deleted_at IS NULL
                   -- Accounts suspended by moderation drop out of the feed. Silenced ones only
                   -- appear to those who already follow them (same path as the OR below).
                   AND c.suspended_at IS NULL
                   AND (
                        oe.citizen_id = $1
                        OR (
                             c.handle IS NOT NULL
                             AND c.silenced_at IS NULL
                             AND EXISTS (
                               SELECT 1 FROM federation_follow f
                                WHERE f.citizen_id = $1
                                  AND f.direction = 'outbound'
                                  AND f.remote_actor_url = $3 || '/actors/' || c.handle))
                        OR (
                             c.handle IS NOT NULL
                             AND c.silenced_at IS NOT NULL
                             AND EXISTS (
                               SELECT 1 FROM federation_follow f
                                WHERE f.citizen_id = $1
                                  AND f.direction = 'outbound'
                                  AND f.remote_actor_url = $3 || '/actors/' || c.handle)))
                UNION ALL
                SELECT t.object_uri,
                       t.actor_handle,
                       t.actor_display_name,
                       t.actor_avatar_url,
                       t.content_html,
                       t.published_at,
                       true,
                       t.in_reply_to_uri,
                       t.sensitive,
                       t.spoiler_text,
                       t.edited_at,
                       t.actor_url                                            AS author_actor_url
                  FROM federation_timeline_entry t
                 WHERE t.deleted_at IS NULL
                   AND EXISTS (SELECT 1 FROM federation_follow f
                                WHERE f.citizen_id = $1
                                  AND f.direction = 'outbound'
                                  AND f.remote_actor_url = t.actor_url)
               ) AS u
                 ORDER BY u.object_uri, u.is_remote ASC
               ) AS feed
         WHERE (feed.author_actor_url IS NULL
                OR feed.author_actor_url NOT IN (
                       SELECT target_actor_url FROM actor_mute
                        WHERE citizen_id = $1))
           AND (feed.author_actor_url IS NULL
                OR feed.author_actor_url NOT IN (
                       SELECT target_actor_url FROM actor_block
                        WHERE citizen_id = $1))
           AND (feed.author_actor_url IS NULL
                OR lower(substring(feed.author_actor_url FROM '^https?://([^/]+)'))
                       NOT IN (SELECT domain FROM domain_block
                                WHERE citizen_id = $1))
           -- Server-wide domain policy (migration 0508). 'suspend' = corte
           -- total; 'silence' = only those who already follow keep seeing it.
           AND (feed.author_actor_url IS NULL
                OR lower(substring(feed.author_actor_url FROM '^https?://([^/]+)'))
                       NOT IN (SELECT domain FROM server_domain_block
                                WHERE severity = 'suspend'))
           AND (feed.author_actor_url IS NULL
                OR NOT EXISTS (
                     SELECT 1 FROM server_domain_block sb
                      WHERE sb.severity = 'silence'
                        AND sb.domain = lower(substring(feed.author_actor_url FROM '^https?://([^/]+)'))
                        AND NOT EXISTS (
                              SELECT 1 FROM federation_follow ff
                               WHERE ff.citizen_id = $1
                                 AND ff.direction = 'outbound'
                                 AND ff.remote_actor_url = feed.author_actor_url)))
           AND NOT EXISTS (
                   SELECT 1 FROM content_filter cf
                    WHERE cf.citizen_id = $1
                      AND 'home' = ANY(cf.context)
                      AND (cf.expires_at IS NULL OR cf.expires_at > now())
                      AND position(lower(cf.phrase) in lower(feed.content_html)) > 0)
         ORDER BY feed.published_at DESC
         LIMIT $4 OFFSET $5
        ",
    )
    .bind(citizen_id)
    .bind(media_base_url.trim_end_matches('/'))
    .bind(public_origin.trim_end_matches('/'))
    .bind(limit)
    .bind(offset)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(FeedItemDto::from).collect())
}

/// Does ANY local citizen follow this remote actor (ACK'd or pending)? Gates whether an inbound
/// `Create(Note)` is worth storing — we are a feed for our members, not a fediverse archive.
pub async fn anyone_follows(db: &PgPool, remote_actor_url: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(
        r"
        SELECT EXISTS (SELECT 1 FROM federation_follow
                        WHERE direction = 'outbound' AND remote_actor_url = $1)
        ",
    )
    .bind(remote_actor_url)
    .fetch_one(db)
    .await
}

/// Store a remote Note ONCE per `object_uri` (re-deliveries and fan-in from multiple followers
/// are no-ops). Content must already be sanitized + size-capped by the caller. 0.18.0: also
/// stores `in_reply_to_uri`, `sensitive` and `spoiler_text` (CW) — all NULL/false for legacy
/// callers that pass defaults.
#[allow(clippy::too_many_arguments)]
pub async fn upsert_timeline_entry(
    db: &PgPool,
    object_uri: &str,
    actor_url: &str,
    actor_handle: &str,
    actor_display_name: Option<&str>,
    actor_avatar_url: Option<&str>,
    content_html: &str,
    published_at: DateTime<Utc>,
    received_at: DateTime<Utc>,
    in_reply_to_uri: Option<&str>,
    sensitive: bool,
    spoiler_text: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r"
        INSERT INTO federation_timeline_entry
            (id, object_uri, actor_url, actor_handle, actor_display_name, actor_avatar_url,
             content_html, published_at, received_at,
             in_reply_to_uri, sensitive, spoiler_text)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
        ON CONFLICT (object_uri) DO NOTHING
        ",
    )
    .bind(Uuid::now_v7())
    .bind(object_uri)
    .bind(actor_url)
    .bind(actor_handle)
    .bind(actor_display_name)
    .bind(actor_avatar_url)
    .bind(content_html)
    .bind(published_at)
    .bind(received_at)
    .bind(in_reply_to_uri)
    .bind(sensitive)
    .bind(spoiler_text)
    .execute(db)
    .await?;
    Ok(())
}

/// Public timeline of Notes tagged with a hashtag. Returns local outbox
/// entries + remote timeline entries whose `object_uri` is indexed under
/// `tag_normalized`. Public (no citizen scoping): `liked_by_me`/`boosted_by_me`
/// always come back false.
pub async fn list_hashtag_timeline(
    db: &PgPool,
    tag_normalized: &str,
    media_base_url: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<FeedItemDto>, sqlx::Error> {
    let rows = sqlx::query_as::<_, FeedRow>(
        r"
        WITH tag_uris AS (
            SELECT object_uri FROM note_hashtag WHERE tag_normalized = $1
        )
        SELECT feed.object_uri,
               feed.author_handle,
               feed.author_display_name,
               feed.author_avatar_url,
               feed.content_html,
               feed.published_at,
               feed.is_remote,
               feed.in_reply_to_uri,
               feed.sensitive,
               feed.spoiler_text,
               feed.edited_at,
               (SELECT count(*) FROM federation_reaction r
                 WHERE r.object_uri = feed.object_uri AND r.kind = 'like')
             + (SELECT count(*) FROM federation_remote_reaction rr
                 WHERE rr.object_uri = feed.object_uri AND rr.kind = 'like')  AS like_count,
               (SELECT count(*) FROM federation_reaction r
                 WHERE r.object_uri = feed.object_uri AND r.kind = 'boost')
             + (SELECT count(*) FROM federation_remote_reaction rr
                 WHERE rr.object_uri = feed.object_uri AND rr.kind = 'boost') AS boost_count,
               false AS liked_by_me,
               false AS boosted_by_me
          FROM (
                -- DISTINCT ON: the same note may exist locally (outbox) AND ingested via
                -- federacao (loop no proprio dominio). Duplicata derruba o {#each}
                -- chaveado do front (each_key_duplicate) -- prefere a linha local.
                SELECT DISTINCT ON (u.object_uri) u.*
                  FROM (
                SELECT COALESCE(oe.payload->'object'->>'id', oe.activity_id) AS object_uri,
                       '@' || COALESCE(c.handle, 'u-' || replace(c.id::text, '-', ''))
                                                                             AS author_handle,
                       c.display_name                                        AS author_display_name,
                       CASE WHEN c.avatar_object_key IS NOT NULL AND $2 <> ''
                            THEN $2 || '/' || c.avatar_object_key END        AS author_avatar_url,
                       COALESCE(oe.payload->'object'->>'content', '')        AS content_html,
                       oe.created_at                                         AS published_at,
                       false                                                 AS is_remote,
                       oe.in_reply_to_uri                                    AS in_reply_to_uri,
                       oe.sensitive                                          AS sensitive,
                       oe.spoiler_text                                       AS spoiler_text,
                       oe.edited_at                                          AS edited_at
                  FROM federation_outbox_entry oe
                  JOIN citizen c ON c.id = oe.citizen_id
                 WHERE oe.kind = 'Create'
                   AND oe.deleted_at IS NULL
                   AND COALESCE(oe.payload->'object'->>'id', oe.activity_id)
                       IN (SELECT object_uri FROM tag_uris)
                UNION ALL
                SELECT t.object_uri,
                       t.actor_handle,
                       t.actor_display_name,
                       t.actor_avatar_url,
                       t.content_html,
                       t.published_at,
                       true,
                       t.in_reply_to_uri,
                       t.sensitive,
                       t.spoiler_text,
                       t.edited_at
                  FROM federation_timeline_entry t
                 WHERE t.deleted_at IS NULL
                   AND t.object_uri IN (SELECT object_uri FROM tag_uris)
               ) AS u
                 ORDER BY u.object_uri, u.is_remote ASC
               ) AS feed
         ORDER BY feed.published_at DESC
         LIMIT $3 OFFSET $4
        ",
    )
    .bind(tag_normalized)
    .bind(media_base_url.trim_end_matches('/'))
    .bind(limit)
    .bind(offset)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(FeedItemDto::from).collect())
}

/// Persist an extracted hashtag reference (idempotent). Mirrors
/// `dsoc_auth::queries::insert_note_hashtag` but callable from the gateway inbox handler
/// (which does not depend on the auth crate).
pub async fn upsert_hashtag(
    db: &PgPool,
    object_uri: &str,
    tag_normalized: &str,
    tag_original: &str,
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r"
        INSERT INTO note_hashtag (id, object_uri, tag_normalized, tag_original, created_at)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (object_uri, tag_normalized) DO NOTHING
        ",
    )
    .bind(Uuid::now_v7())
    .bind(object_uri)
    .bind(tag_normalized)
    .bind(tag_original)
    .bind(now)
    .execute(db)
    .await?;
    Ok(())
}

/// Persist an extracted mention reference (idempotent).
pub async fn upsert_mention(
    db: &PgPool,
    object_uri: &str,
    mentioned_actor_url: &str,
    mentioned_handle: &str,
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r"
        INSERT INTO note_mention (id, object_uri, mentioned_actor_url, mentioned_handle, created_at)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (object_uri, mentioned_actor_url) DO NOTHING
        ",
    )
    .bind(Uuid::now_v7())
    .bind(object_uri)
    .bind(mentioned_actor_url)
    .bind(mentioned_handle)
    .bind(now)
    .execute(db)
    .await?;
    Ok(())
}

/// Descendants of a Note by URI (children, grandchildren, …) — recursive on `in_reply_to_uri`,
/// depth capped at 40 so a pathological thread cannot blow the query. The root note itself is
/// included as the first row. Ancestors (walking `in_reply_to_uri` upward) are a follow-up in
/// 0.19.0; today the front-end can look up the parent one hop at a time when needed.
/// Returns the same `FeedItemDto` shape as `list_feed`, so the front reuses its renderer.
pub async fn list_thread_context(
    db: &PgPool,
    root_uri: &str,
    citizen_id: Uuid,
    media_base_url: &str,
) -> Result<Vec<FeedItemDto>, sqlx::Error> {
    let rows = sqlx::query_as::<_, FeedRow>(
        r"
        WITH RECURSIVE thread(object_uri, depth) AS (
            SELECT $1::text, 0
            UNION ALL
            SELECT child_uri, thread.depth + 1
              FROM thread
              JOIN LATERAL (
                    SELECT COALESCE(oe.payload->'object'->>'id', oe.activity_id) AS child_uri
                      FROM federation_outbox_entry oe
                     WHERE oe.in_reply_to_uri = thread.object_uri
                       AND oe.deleted_at IS NULL
                    UNION ALL
                    SELECT t.object_uri
                      FROM federation_timeline_entry t
                     WHERE t.in_reply_to_uri = thread.object_uri
                       AND t.deleted_at IS NULL
              ) c ON true
             WHERE thread.depth < 40
        )
        SELECT feed.object_uri,
               feed.author_handle,
               feed.author_display_name,
               feed.author_avatar_url,
               feed.content_html,
               feed.published_at,
               feed.is_remote,
               feed.in_reply_to_uri,
               feed.sensitive,
               feed.spoiler_text,
               feed.edited_at,
               (SELECT count(*) FROM federation_reaction r
                 WHERE r.object_uri = feed.object_uri AND r.kind = 'like')
             + (SELECT count(*) FROM federation_remote_reaction rr
                 WHERE rr.object_uri = feed.object_uri AND rr.kind = 'like')  AS like_count,
               (SELECT count(*) FROM federation_reaction r
                 WHERE r.object_uri = feed.object_uri AND r.kind = 'boost')
             + (SELECT count(*) FROM federation_remote_reaction rr
                 WHERE rr.object_uri = feed.object_uri AND rr.kind = 'boost') AS boost_count,
               EXISTS (SELECT 1 FROM federation_reaction r
                        WHERE r.citizen_id = $2 AND r.object_uri = feed.object_uri
                          AND r.kind = 'like')  AS liked_by_me,
               EXISTS (SELECT 1 FROM federation_reaction r
                        WHERE r.citizen_id = $2 AND r.object_uri = feed.object_uri
                          AND r.kind = 'boost') AS boosted_by_me
          FROM (
                -- DISTINCT ON: the same note may exist locally (outbox) AND ingested via
                -- federacao (loop no proprio dominio). Duplicata derruba o {#each}
                -- chaveado do front (each_key_duplicate) -- prefere a linha local.
                SELECT DISTINCT ON (u.object_uri) u.*
                  FROM (
                SELECT COALESCE(oe.payload->'object'->>'id', oe.activity_id) AS object_uri,
                       '@' || COALESCE(c.handle, 'u-' || replace(c.id::text, '-', ''))
                                                                             AS author_handle,
                       c.display_name                                        AS author_display_name,
                       CASE WHEN c.avatar_object_key IS NOT NULL AND $3 <> ''
                            THEN $3 || '/' || c.avatar_object_key END        AS author_avatar_url,
                       COALESCE(oe.payload->'object'->>'content', '')        AS content_html,
                       oe.created_at                                         AS published_at,
                       false                                                 AS is_remote,
                       oe.in_reply_to_uri                                    AS in_reply_to_uri,
                       oe.sensitive                                          AS sensitive,
                       oe.spoiler_text                                       AS spoiler_text,
                       oe.edited_at                                          AS edited_at
                  FROM federation_outbox_entry oe
                  JOIN citizen c ON c.id = oe.citizen_id
                 WHERE oe.kind = 'Create'
                   AND oe.deleted_at IS NULL
                   AND COALESCE(oe.payload->'object'->>'id', oe.activity_id)
                       IN (SELECT object_uri FROM thread)
                UNION ALL
                SELECT t.object_uri,
                       t.actor_handle,
                       t.actor_display_name,
                       t.actor_avatar_url,
                       t.content_html,
                       t.published_at,
                       true,
                       t.in_reply_to_uri,
                       t.sensitive,
                       t.spoiler_text,
                       t.edited_at
                  FROM federation_timeline_entry t
                 WHERE t.deleted_at IS NULL
                   AND t.object_uri IN (SELECT object_uri FROM thread)
               ) AS u
                 ORDER BY u.object_uri, u.is_remote ASC
               ) AS feed
         ORDER BY feed.published_at ASC
        ",
    )
    .bind(root_uri)
    .bind(citizen_id)
    .bind(media_base_url.trim_end_matches('/'))
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(FeedItemDto::from).collect())
}

/// Is `object_uri` one of OUR objects (a Note id or activity id from `federation_outbox_entry`)?
/// Gates inbound `Like`/`Announce` so we don't accumulate reactions over arbitrary third-party URIs.
pub async fn is_our_object(db: &PgPool, object_uri: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(
        r"
        SELECT EXISTS (SELECT 1 FROM federation_outbox_entry
                        WHERE activity_id = $1 OR payload->'object'->>'id' = $1)
        ",
    )
    .bind(object_uri)
    .fetch_one(db)
    .await
}

/// Record a remote actor's reaction over one of our objects. Re-delivery is a no-op.
pub async fn upsert_remote_reaction(
    db: &PgPool,
    remote_actor_url: &str,
    object_uri: &str,
    kind: &str,
    activity_id: &str,
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r"
        INSERT INTO federation_remote_reaction
            (id, remote_actor_url, object_uri, kind, activity_id, created_at)
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (remote_actor_url, object_uri, kind) DO NOTHING
        ",
    )
    .bind(Uuid::now_v7())
    .bind(remote_actor_url)
    .bind(object_uri)
    .bind(kind)
    .bind(activity_id)
    .bind(now)
    .execute(db)
    .await?;
    Ok(())
}

/// Remove a remote reaction on `Undo(Like|Announce)`. Matches either by the original activity id
/// (what Mastodon echoes in the inner object) or by (actor, object, kind) when only the target
/// URI is known. Scoped to the SIGNER so one instance cannot undo another's reaction.
pub async fn delete_remote_reaction(
    db: &PgPool,
    remote_actor_url: &str,
    kind: &str,
    activity_id: Option<&str>,
    object_uri: Option<&str>,
) -> Result<u64, sqlx::Error> {
    let r = sqlx::query(
        r"
        DELETE FROM federation_remote_reaction
         WHERE remote_actor_url = $1
           AND kind = $2
           AND (($3::text IS NOT NULL AND activity_id = $3)
                OR ($4::text IS NOT NULL AND object_uri = $4))
        ",
    )
    .bind(remote_actor_url)
    .bind(kind)
    .bind(activity_id)
    .bind(object_uri)
    .execute(db)
    .await?;
    Ok(r.rows_affected())
}

/// Read the citizen's existing reaction over an object, if any. Returns the activity id we
/// emitted when the reaction was created (needed to build the `Undo` on toggle-off).
pub async fn find_local_reaction(
    db: &PgPool,
    citizen_id: Uuid,
    object_uri: &str,
    kind: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar::<_, String>(
        r"
        SELECT activity_id FROM federation_reaction
         WHERE citizen_id = $1 AND object_uri = $2 AND kind = $3
        ",
    )
    .bind(citizen_id)
    .bind(object_uri)
    .bind(kind)
    .fetch_optional(db)
    .await
}

/// Persist a local reaction (idempotent — a concurrent double-toggle just no-ops).
pub async fn insert_local_reaction(
    db: &PgPool,
    citizen_id: Uuid,
    object_uri: &str,
    kind: &str,
    activity_id: &str,
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r"
        INSERT INTO federation_reaction (id, citizen_id, object_uri, kind, activity_id, created_at)
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (citizen_id, object_uri, kind) DO NOTHING
        ",
    )
    .bind(Uuid::now_v7())
    .bind(citizen_id)
    .bind(object_uri)
    .bind(kind)
    .bind(activity_id)
    .bind(now)
    .execute(db)
    .await?;
    Ok(())
}

/// Remove a local reaction (the toggle-off half).
pub async fn delete_local_reaction(
    db: &PgPool,
    citizen_id: Uuid,
    object_uri: &str,
    kind: &str,
) -> Result<u64, sqlx::Error> {
    let r = sqlx::query(
        r"
        DELETE FROM federation_reaction
         WHERE citizen_id = $1 AND object_uri = $2 AND kind = $3
        ",
    )
    .bind(citizen_id)
    .bind(object_uri)
    .bind(kind)
    .execute(db)
    .await?;
    Ok(r.rows_affected())
}

/// Combined reaction count for an object: local citizens + remote actors.
pub async fn count_reactions(
    db: &PgPool,
    object_uri: &str,
    kind: &str,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        r"
        SELECT (SELECT count(*) FROM federation_reaction
                 WHERE object_uri = $1 AND kind = $2)
             + (SELECT count(*) FROM federation_remote_reaction
                 WHERE object_uri = $1 AND kind = $2)
        ",
    )
    .bind(object_uri)
    .bind(kind)
    .fetch_one(db)
    .await
}

/// If `object_uri` is a REMOTE note we hold, return its author's Actor URL (delivery target for
/// outbound `Like`/`Announce`/`Undo`). `None` ⇒ the object is local (or unknown): nothing to deliver.
pub async fn find_timeline_actor(
    db: &PgPool,
    object_uri: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar::<_, String>(
        r"
        SELECT actor_url FROM federation_timeline_entry WHERE object_uri = $1
        ",
    )
    .bind(object_uri)
    .fetch_optional(db)
    .await
}

// --- HTML sanitization ---------------------------------------------------------------------

/// Tags kept from remote Note content. Everything else (script, iframe, img, forms, …) is
/// dropped wholesale; `script`/`style` also drop their inner text.
const ALLOWED_TAGS: &[&str] = &[
    "p",
    "br",
    "a",
    "span",
    "em",
    "strong",
    "i",
    "b",
    "u",
    "s",
    "ul",
    "ol",
    "li",
    "blockquote",
    "code",
    "pre",
    "small",
    "sub",
    "sup",
];

/// Allowlist-sanitize remote HTML before it is persisted (defense-in-depth: the front should
/// still render it inside a sanitizing component). Behavior:
///
/// * only [`ALLOWED_TAGS`] survive; all attributes are stripped except `href` on `<a>` (which
///   must be `http(s)://` and gets `rel="nofollow noopener noreferrer" target="_blank"`);
/// * `<script>`/`<style>` are removed INCLUDING their text content;
/// * text nodes pass through untouched (they arrive entity-encoded from the remote);
/// * a dangling `<` without a matching `>` is escaped literally.
pub fn sanitize_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        rest = &rest[lt..];
        let Some(gt) = rest.find('>') else {
            // Unterminated tag — neutralize the remainder literally and stop.
            out.push_str(&rest.replace('<', "&lt;"));
            rest = "";
            break;
        };
        let tag_raw = &rest[1..gt];
        rest = &rest[gt + 1..];
        let is_closing = tag_raw.starts_with('/');
        let name: String = tag_raw
            .trim_start_matches('/')
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase();
        if name.is_empty() {
            // Comment / doctype / junk — drop.
            continue;
        }
        if name == "script" || name == "style" {
            if !is_closing {
                // Drop everything through the matching close tag (or to the end).
                let close = format!("</{name}");
                if let Some(pos) = rest.to_ascii_lowercase().find(&close) {
                    let after = &rest[pos..];
                    rest = after.find('>').map_or("", |g| &after[g + 1..]);
                } else {
                    rest = "";
                }
            }
            continue;
        }
        if !ALLOWED_TAGS.contains(&name.as_str()) {
            continue;
        }
        if is_closing {
            out.push_str("</");
            out.push_str(&name);
            out.push('>');
        } else if name == "br" {
            out.push_str("<br>");
        } else if name == "a" {
            match extract_href(tag_raw) {
                Some(href) => {
                    out.push_str("<a href=\"");
                    out.push_str(&escape_attr(&href));
                    out.push_str("\" rel=\"nofollow noopener noreferrer\" target=\"_blank\">");
                }
                None => out.push_str("<a>"),
            }
        } else {
            out.push('<');
            out.push_str(&name);
            out.push('>');
        }
    }
    out.push_str(rest);
    out
}

/// Pull a safe `href` out of a raw `<a …>` tag body. Only absolute `http(s)://` URLs survive —
/// `javascript:`, `data:`, protocol-relative and site-relative values are all dropped.
fn extract_href(tag_raw: &str) -> Option<String> {
    let lower = tag_raw.to_ascii_lowercase();
    let idx = lower.find("href")?;
    let after = tag_raw
        .get(idx + 4..)?
        .trim_start()
        .strip_prefix('=')?
        .trim_start();
    let value = if let Some(r) = after.strip_prefix('"') {
        r.split('"').next()?
    } else if let Some(r) = after.strip_prefix('\'') {
        r.split('\'').next()?
    } else {
        after.split_whitespace().next()?
    };
    let value = value.trim();
    if value.starts_with("https://") || value.starts_with("http://") {
        Some(value.to_owned())
    } else {
        None
    }
}

fn escape_attr(v: &str) -> String {
    v.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Truncate a string to at most `max` BYTES without splitting a UTF-8 char.
pub fn truncate_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_keeps_allowed_and_drops_script() {
        let dirty = r#"<p>Olá <strong>mundo</strong></p><script>alert(1)</script><p>fim</p>"#;
        assert_eq!(
            sanitize_html(dirty),
            "<p>Olá <strong>mundo</strong></p><p>fim</p>"
        );
    }

    #[test]
    fn sanitize_strips_event_handlers_and_unknown_tags() {
        let dirty =
            r#"<p onclick="evil()">x</p><img src=x onerror=evil()><iframe src="a"></iframe>"#;
        assert_eq!(sanitize_html(dirty), "<p>x</p>");
    }

    #[test]
    fn sanitize_rewrites_links_and_blocks_javascript_scheme() {
        let ok = sanitize_html(r#"<a href="https://pop.coop/x" class="u-url">link</a>"#);
        assert_eq!(
            ok,
            r#"<a href="https://pop.coop/x" rel="nofollow noopener noreferrer" target="_blank">link</a>"#
        );
        let bad = sanitize_html(r#"<a href="javascript:evil()">x</a>"#);
        assert_eq!(bad, "<a>x</a>");
    }

    #[test]
    fn sanitize_escapes_dangling_lt() {
        assert_eq!(sanitize_html("a < b"), "a &lt; b");
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        let s = "ação"; // multi-byte chars
        let t = truncate_bytes(s, 3);
        assert!(t.len() <= 3);
        assert!(s.starts_with(t));
        assert_eq!(truncate_bytes("abc", 10), "abc");
    }
}

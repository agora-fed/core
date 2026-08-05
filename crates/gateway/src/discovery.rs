//! # Search / trends / directory — the discovery surface (Session G, 0.18.0-rc2).
//!
//! Complements the timeline endpoints: this is how a user finds *new* people
//! and topics before they exist in their feed. Reuses the existing tables
//! (`note_hashtag`, `citizen`, `federation_outbox_entry`, `federation_reaction`)
//! — no new schema. Endpoints all runtime-unchecked `sqlx::query*`.
//!
//! ## Signals
//!
//! * Hashtag search: prefix match on `tag_normalized`, grouped by count.
//! * Mention search: local citizens (`is_public = true`) matched on handle
//!   substring OR case-insensitive display_name substring.
//! * Trending hashtags: distinct-object count in the last 24h.
//! * Directory: public citizens ordered by "profile completeness"
//!   (avatar + display_name + bio all set = full-strength).
//! * Follow suggestions: public local citizens the caller does not already
//!   follow, ordered by their recent activity in the outbox.

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct HashtagHit {
    pub tag_normalized: String,
    pub tag_original: String,
    pub note_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MentionHit {
    pub handle: String,
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub avatar_url: Option<String>,
    pub actor_url: String,
}

#[derive(Debug, Serialize)]
pub struct NoteHit {
    pub object_uri: String,
    pub author_handle: String,
    pub author_display_name: Option<String>,
    pub author_avatar_url: Option<String>,
    pub content_html: String,
    pub published_at: DateTime<Utc>,
    pub is_remote: bool,
}

pub async fn hashtags_matching(
    db: &PgPool,
    q: &str,
    limit: i64,
) -> Result<Vec<HashtagHit>, sqlx::Error> {
    // Normalize the query the same way we normalized the stored tags: strip
    // the leading '#' if the caller included it, then reuse the extractor's
    // normalize on `#<q>` to fold diacritics + lowercase.
    let cleaned = q.trim().trim_start_matches('#');
    if cleaned.is_empty() {
        return Ok(Vec::new());
    }
    let normalized = dsoc_federation::extract_hashtags(&format!("#{cleaned}"))
        .into_iter()
        .next()
        .map(|h| h.normalized)
        .unwrap_or_else(|| cleaned.to_lowercase());
    let pattern = format!("{normalized}%");
    sqlx::query_as::<_, (String, String, i64)>(
        r"
        SELECT tag_normalized,
               MIN(tag_original) AS tag_original,
               COUNT(DISTINCT object_uri) AS note_count
          FROM note_hashtag
         WHERE tag_normalized LIKE $1
         GROUP BY tag_normalized
         ORDER BY note_count DESC, tag_normalized
         LIMIT $2
        ",
    )
    .bind(&pattern)
    .bind(limit)
    .fetch_all(db)
    .await
    .map(|rows| {
        rows.into_iter()
            .map(|(tag_normalized, tag_original, note_count)| HashtagHit {
                tag_normalized,
                tag_original,
                note_count,
            })
            .collect()
    })
}

pub async fn mentions_matching(
    db: &PgPool,
    q: &str,
    public_origin: &str,
    media_base_url: &str,
    limit: i64,
) -> Result<Vec<MentionHit>, sqlx::Error> {
    let cleaned = q.trim().trim_start_matches('@');
    if cleaned.is_empty() {
        return Ok(Vec::new());
    }
    let pattern = format!("%{}%", cleaned.to_lowercase());
    let handle_prefix = format!("{}%", cleaned.to_lowercase());
    let rows: Vec<(String, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        r"
        SELECT c.handle,
               c.display_name,
               c.bio,
               CASE WHEN c.avatar_object_key IS NOT NULL AND $3 <> ''
                    THEN $3 || '/' || c.avatar_object_key END AS avatar_url
          FROM citizen c
         WHERE c.is_public = true
           AND c.handle IS NOT NULL
           AND (lower(c.handle) LIKE $2
                OR (c.display_name IS NOT NULL AND lower(c.display_name) LIKE $1))
         ORDER BY
             CASE WHEN lower(c.handle) LIKE $2 THEN 0 ELSE 1 END,
             c.display_name
         LIMIT $4
        ",
    )
    .bind(&pattern)
    .bind(&handle_prefix)
    .bind(media_base_url.trim_end_matches('/'))
    .bind(limit)
    .fetch_all(db)
    .await?;
    let origin = public_origin.trim_end_matches('/').to_owned();
    Ok(rows
        .into_iter()
        .map(|(handle, display_name, bio, avatar_url)| MentionHit {
            actor_url: format!("{origin}/actors/{handle}"),
            handle,
            display_name,
            bio,
            avatar_url,
        })
        .collect())
}

/// Who the citizen FOLLOWS, filtered by a substring of the actor's URL — the
/// likeliest suggestions for a mention, including remote accounts the local search
/// would never find (it is what makes `@pedroz…` suggest `pedrozambarda@organica.social`).
/// `display_name`/`avatar` stay None for remote ones; the handler enriches them when the
/// same actor is also a local citizen.
pub async fn followed_mentions_matching(
    db: &PgPool,
    citizen_id: Uuid,
    q: &str,
    limit: i64,
) -> Result<Vec<MentionHit>, sqlx::Error> {
    let cleaned = q.trim().trim_start_matches('@');
    if cleaned.is_empty() {
        return Ok(Vec::new());
    }
    let pattern = format!("%{}%", cleaned.to_lowercase());
    let rows: Vec<(String,)> = sqlx::query_as(
        r"
        SELECT f.remote_actor_url
          FROM federation_follow f
         WHERE f.citizen_id = $1
           AND f.direction = 'outbound'
           AND lower(f.remote_actor_url) LIKE $2
         ORDER BY f.created_at DESC
         LIMIT $3
        ",
    )
    .bind(citizen_id)
    .bind(&pattern)
    .bind(limit)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(actor_url,)| {
            let handle = handle_from_actor_url(&actor_url)?;
            Some(MentionHit {
                handle,
                display_name: None,
                bio: None,
                avatar_url: None,
                actor_url,
            })
        })
        .collect())
}

/// `https://host/users/name` (or `/actors/name`) → `name@host`. The fediverse
/// convention for deriving a displayable handle without fetching the actor.
fn handle_from_actor_url(u: &str) -> Option<String> {
    let rest = u.strip_prefix("https://")?;
    let (host, path) = rest.split_once('/')?;
    let user = path.trim_end_matches('/').rsplit('/').next()?;
    if user.is_empty() || host.is_empty() {
        return None;
    }
    Some(format!("{user}@{host}"))
}

pub async fn notes_matching(
    db: &PgPool,
    q: &str,
    media_base_url: &str,
    limit: i64,
) -> Result<Vec<NoteHit>, sqlx::Error> {
    if q.trim().is_empty() {
        return Ok(Vec::new());
    }
    let pattern = format!("%{}%", q.trim().to_lowercase());
    // Union of local outbox + remote timeline entries; ILIKE on content.
    // FTS+tsvector deferred to a later migration — this is fine for the
    // corpus size we have while we bootstrap the corpus itself.
    sqlx::query_as::<_, (
        String,
        String,
        Option<String>,
        Option<String>,
        String,
        DateTime<Utc>,
        bool,
    )>(
        r"
        SELECT * FROM (
          SELECT COALESCE(oe.payload->'object'->>'id', oe.activity_id)          AS object_uri,
                 '@' || COALESCE(c.handle, 'u-' || replace(c.id::text, '-', ''))
                                                                                 AS author_handle,
                 c.display_name                                                  AS author_display_name,
                 CASE WHEN c.avatar_object_key IS NOT NULL AND $2 <> ''
                      THEN $2 || '/' || c.avatar_object_key END                  AS author_avatar_url,
                 COALESCE(oe.payload->'object'->>'content', '')                  AS content_html,
                 oe.created_at                                                   AS published_at,
                 false                                                           AS is_remote
            FROM federation_outbox_entry oe
            JOIN citizen c ON c.id = oe.citizen_id
           WHERE oe.kind = 'Create'
             AND oe.deleted_at IS NULL
             AND lower(COALESCE(oe.payload->'object'->>'content', '')) LIKE $1
          UNION ALL
          SELECT t.object_uri,
                 t.actor_handle,
                 t.actor_display_name,
                 t.actor_avatar_url,
                 t.content_html,
                 t.published_at,
                 true
            FROM federation_timeline_entry t
           WHERE t.deleted_at IS NULL
             AND lower(t.content_html) LIKE $1
        ) AS hits
        ORDER BY hits.published_at DESC
        LIMIT $3
        ",
    )
    .bind(&pattern)
    .bind(media_base_url.trim_end_matches('/'))
    .bind(limit)
    .fetch_all(db)
    .await
    .map(|rows| {
        rows.into_iter()
            .map(
                |(
                    object_uri,
                    author_handle,
                    author_display_name,
                    author_avatar_url,
                    content_html,
                    published_at,
                    is_remote,
                )| NoteHit {
                    object_uri,
                    author_handle,
                    author_display_name,
                    author_avatar_url,
                    content_html,
                    published_at,
                    is_remote,
                },
            )
            .collect()
    })
}

pub async fn trending_hashtags(
    db: &PgPool,
    window_hours: i64,
    limit: i64,
) -> Result<Vec<HashtagHit>, sqlx::Error> {
    let since = Utc::now() - Duration::hours(window_hours);
    // 0.26.19: excludes 'banned' hashtags and injects 'promoted' ones with a synthetic count
    // so they appear even without volume.
    sqlx::query_as::<_, (String, String, i64)>(
        r"
        WITH real AS (
            SELECT tag_normalized,
                   MIN(tag_original) AS tag_original,
                   COUNT(DISTINCT object_uri) AS note_count
              FROM note_hashtag
             WHERE created_at >= $1
               AND tag_normalized NOT IN (
                     SELECT tag FROM hashtag_moderation WHERE state = 'banned')
             GROUP BY tag_normalized
        ),
        promoted AS (
            SELECT hm.tag AS tag_normalized,
                   hm.tag AS tag_original,
                   COALESCE(r.note_count, 0) AS note_count
              FROM hashtag_moderation hm
              LEFT JOIN real r ON r.tag_normalized = hm.tag
             WHERE hm.state = 'promoted'
        )
        SELECT tag_normalized, tag_original, note_count FROM real
        UNION
        SELECT tag_normalized, tag_original, note_count FROM promoted
        ORDER BY note_count DESC, tag_normalized
        LIMIT $2
        ",
    )
    .bind(since)
    .bind(limit)
    .fetch_all(db)
    .await
    .map(|rows| {
        rows.into_iter()
            .map(|(tag_normalized, tag_original, note_count)| HashtagHit {
                tag_normalized,
                tag_original,
                note_count,
            })
            .collect()
    })
}

pub async fn directory(
    db: &PgPool,
    public_origin: &str,
    media_base_url: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<MentionHit>, sqlx::Error> {
    let rows: Vec<(String, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        r"
        SELECT c.handle,
               c.display_name,
               c.bio,
               CASE WHEN c.avatar_object_key IS NOT NULL AND $1 <> ''
                    THEN $1 || '/' || c.avatar_object_key END AS avatar_url
          FROM citizen c
         WHERE c.is_public = true
           AND c.handle IS NOT NULL
         ORDER BY
             (c.avatar_object_key IS NOT NULL)::int DESC,
             (c.display_name IS NOT NULL AND length(c.display_name) > 0)::int DESC,
             (c.bio IS NOT NULL AND length(c.bio) > 0)::int DESC,
             c.created_at DESC
         LIMIT $2 OFFSET $3
        ",
    )
    .bind(media_base_url.trim_end_matches('/'))
    .bind(limit)
    .bind(offset)
    .fetch_all(db)
    .await?;
    let origin = public_origin.trim_end_matches('/').to_owned();
    Ok(rows
        .into_iter()
        .map(|(handle, display_name, bio, avatar_url)| MentionHit {
            actor_url: format!("{origin}/actors/{handle}"),
            handle,
            display_name,
            bio,
            avatar_url,
        })
        .collect())
}

pub async fn follow_suggestions(
    db: &PgPool,
    caller_id: Uuid,
    public_origin: &str,
    media_base_url: &str,
    limit: i64,
) -> Result<Vec<MentionHit>, sqlx::Error> {
    // Public local citizens the caller does not already follow, ordered by
    // recent outbox activity (most-recent Note wins). Excludes self.
    let rows: Vec<(String, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        r"
        SELECT c.handle,
               c.display_name,
               c.bio,
               CASE WHEN c.avatar_object_key IS NOT NULL AND $2 <> ''
                    THEN $2 || '/' || c.avatar_object_key END AS avatar_url
          FROM citizen c
     LEFT JOIN LATERAL (
             SELECT max(oe.created_at) AS last_note
               FROM federation_outbox_entry oe
              WHERE oe.citizen_id = c.id
                AND oe.deleted_at IS NULL
           ) latest ON true
         WHERE c.is_public = true
           AND c.handle IS NOT NULL
           AND c.id <> $1
           AND NOT EXISTS (
                SELECT 1 FROM federation_follow f
                 WHERE f.citizen_id = $1
                   AND f.direction = 'outbound'
                   AND f.remote_actor_url = $3 || '/actors/' || c.handle
             )
         ORDER BY COALESCE(latest.last_note, c.created_at) DESC
         LIMIT $4
        ",
    )
    .bind(caller_id)
    .bind(media_base_url.trim_end_matches('/'))
    .bind(public_origin.trim_end_matches('/'))
    .bind(limit)
    .fetch_all(db)
    .await?;
    let origin = public_origin.trim_end_matches('/').to_owned();
    Ok(rows
        .into_iter()
        .map(|(handle, display_name, bio, avatar_url)| MentionHit {
            actor_url: format!("{origin}/actors/{handle}"),
            handle,
            display_name,
            bio,
            avatar_url,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::handle_from_actor_url;

    #[test]
    fn handle_from_mastodon_and_own_actor_urls() {
        assert_eq!(
            handle_from_actor_url("https://exemplo.social/users/fulano").as_deref(),
            Some("fulano@exemplo.social")
        );
        assert_eq!(
            handle_from_actor_url("https://democracia.social.br/actors/socrates").as_deref(),
            Some("socrates@democracia.social.br")
        );
    }

    #[test]
    fn handle_from_bad_urls_is_none() {
        assert_eq!(handle_from_actor_url("http://sem-tls/users/x"), None);
        assert_eq!(handle_from_actor_url("https://sohost"), None);
        assert_eq!(handle_from_actor_url("https://host/"), None);
    }
}

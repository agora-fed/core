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
        }
    }
}

/// Assemble the citizen's merged feed:
/// (a) local Notes authored by the citizen or by LOCAL citizens they follow (an outbound follow
///     whose `remote_actor_url` is `{public_origin}/actors/{handle}` — that is how a local→local
///     follow lands, since `/me/follow` webfinger-resolves our own instance like any other), and
/// (b) remote Notes (`federation_timeline_entry`) from actors they follow,
/// ordered by `published_at DESC`. Counts merge local + remote reactions.
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
                SELECT COALESCE(oe.payload->'object'->>'id', oe.activity_id) AS object_uri,
                       '@' || COALESCE(c.handle, 'u-' || replace(c.id::text, '-', ''))
                                                                             AS author_handle,
                       c.display_name                                        AS author_display_name,
                       CASE WHEN c.avatar_object_key IS NOT NULL AND $2 <> ''
                            THEN $2 || '/' || c.avatar_object_key END        AS author_avatar_url,
                       COALESCE(oe.payload->'object'->>'content', '')        AS content_html,
                       oe.created_at                                         AS published_at,
                       false                                                 AS is_remote
                  FROM federation_outbox_entry oe
                  JOIN citizen c ON c.id = oe.citizen_id
                 WHERE oe.kind = 'Create'
                   AND (oe.citizen_id = $1
                        OR (c.handle IS NOT NULL AND EXISTS (
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
                       true
                  FROM federation_timeline_entry t
                 WHERE EXISTS (SELECT 1 FROM federation_follow f
                                WHERE f.citizen_id = $1
                                  AND f.direction = 'outbound'
                                  AND f.remote_actor_url = t.actor_url)
               ) AS feed
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
/// are no-ops). Content must already be sanitized + size-capped by the caller.
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
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r"
        INSERT INTO federation_timeline_entry
            (id, object_uri, actor_url, actor_handle, actor_display_name, actor_avatar_url,
             content_html, published_at, received_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
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
    .execute(db)
    .await?;
    Ok(())
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
    "p", "br", "a", "span", "em", "strong", "i", "b", "u", "s", "ul", "ol", "li", "blockquote",
    "code", "pre", "small", "sub", "sup",
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
    let after = tag_raw.get(idx + 4..)?.trim_start().strip_prefix('=')?.trim_start();
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
        let dirty = r#"<p onclick="evil()">x</p><img src=x onerror=evil()><iframe src="a"></iframe>"#;
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

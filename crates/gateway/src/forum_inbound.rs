//! Activities a remote server sends to a forum's Group actor (FEP-1b12): contributions, votes,
//! edits and deletions. The caller has already verified the HTTP signature and that the signer is
//! the declared `actor`; [`lookup_by_handle`](crate::forum_federation::lookup_by_handle) only
//! finds public federated forums, so nothing here can reach a restricted one.
//!
//! Federated participation is displayed but never fires a dispatch threshold (0540): nothing here
//! calls the forums service's interaction hook, and remote votes stay out of the score.

use axum::http::StatusCode;
use reqwest::Url;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use dsoc_forums::domain::{MAX_BODY_LEN, MAX_COMMENT_LEN, MAX_TITLE_LEN};

use crate::forum_federation::{lookup_by_handle, FedForum};

const SYSTEM_SUBJECT: &str = "system:fediverse";

/// Instance policy. Unset means inbound is off and a forum only takes Follow and Undo Follow.
#[derive(Debug, Clone, Default)]
pub struct InboundConfig {
    /// `FORUM_INBOUND_ENABLED=true`.
    pub enabled: bool,
    /// `FORUM_INBOUND_TRUSTED_HOSTS`: comma-separated hosts whose accounts skip the queue.
    pub trusted_hosts: Vec<String>,
}

impl InboundConfig {
    /// Read the policy from the environment.
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("FORUM_INBOUND_ENABLED").is_ok_and(|v| v == "true"),
            trusted_hosts: std::env::var("FORUM_INBOUND_TRUSTED_HOSTS")
                .unwrap_or_default()
                .split(',')
                .map(|h| h.trim().to_ascii_lowercase())
                .filter(|h| !h.is_empty())
                .collect(),
        }
    }
}

/// Lowercase host of an https URL.
pub(crate) fn https_host(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    if parsed.scheme() != "https" {
        return None;
    }
    parsed.host_str().map(str::to_ascii_lowercase)
}

fn strings(value: Option<&Value>) -> Vec<&str> {
    match value {
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

fn is_public_uri(uri: &str) -> bool {
    matches!(
        uri,
        "https://www.w3.org/ns/activitystreams#Public" | "as:Public" | "Public"
    )
}

/// Only publicly addressed content may be stored: a Mastodon direct message that mentions the
/// forum must never surface as a topic.
fn is_public(object: &Value) -> bool {
    ["to", "cc"]
        .iter()
        .flat_map(|field| strings(object.get(*field)))
        .any(is_public_uri)
}

fn addressed_to(object: &Value, actor: &str) -> bool {
    let direct = ["to", "cc", "audience"]
        .iter()
        .flat_map(|field| strings(object.get(*field)))
        .any(|uri| uri == actor);
    direct
        || object
            .get("tag")
            .and_then(Value::as_array)
            .is_some_and(|tags| {
                tags.iter().any(|t| {
                    t.get("type").and_then(Value::as_str) == Some("Mention")
                        && t.get("href").and_then(Value::as_str) == Some(actor)
                })
            })
}

fn decode_entities(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

/// Remote HTML reduced to plain text. Forum bodies are escaped when rendered, so no markup needs
/// to survive; this is what keeps a remote `<script>` from ever being more than text.
pub(crate) fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('>') else {
            rest = "";
            break;
        };
        let tag = rest[start + 1..start + end].to_ascii_lowercase();
        if tag.starts_with("br") {
            out.push('\n');
        } else if tag.starts_with("/p") {
            out.push_str("\n\n");
        }
        let after = &rest[start + end + 1..];
        // The contents of these are never text, so they must not leak into the body.
        rest = ["script", "style"]
            .iter()
            .find(|name| tag.starts_with(**name))
            .map_or(after, |name| {
                let lower = after.to_ascii_lowercase();
                lower
                    .find(&format!("</{name}"))
                    .and_then(|i| after[i..].find('>').map(|j| &after[i + j + 1..]))
                    .unwrap_or("")
            });
    }
    out.push_str(rest);
    decode_entities(&out).replace('\0', "").trim().to_owned()
}

/// `@boas-vindas @x@y.z Title` becomes `Title`: mentions that open a Mastodon post are addressing.
fn strip_leading_mentions(text: &str) -> &str {
    let mut rest = text.trim_start();
    while rest.starts_with('@') {
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        rest = rest[end..].trim_start();
    }
    rest
}

fn truncate_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// What a remote `Page`, `Note` or `Article` says, already validated.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Contribution {
    pub object_uri: String,
    pub title: String,
    pub body: String,
    pub in_reply_to: Option<String>,
}

fn object_text(object: &Value) -> String {
    let markdown = object.get("source").and_then(|s| {
        (s.get("mediaType").and_then(Value::as_str) == Some("text/markdown"))
            .then(|| s.get("content").and_then(Value::as_str))
            .flatten()
    });
    let mut body = match markdown {
        Some(text) => text.replace('\0', "").trim().to_owned(),
        None => html_to_text(object.get("content").and_then(Value::as_str).unwrap_or("")),
    };
    // A Lemmy link post carries its link here and may have no text at all.
    let link = object
        .get("attachment")
        .and_then(Value::as_array)
        .and_then(|items| {
            items.iter().find_map(|a| {
                a.get("href")
                    .or_else(|| a.get("url"))
                    .and_then(Value::as_str)
            })
        })
        .filter(|href| href.starts_with("https://") || href.starts_with("http://"));
    if let Some(link) = link {
        if !body.contains(link) {
            if !body.is_empty() {
                body.push_str("\n\n");
            }
            body.push_str(link);
        }
    }
    if let Some(warning) = object
        .get("summary")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
    {
        body = format!("{}\n\n{body}", html_to_text(warning));
    }
    body
}

/// Validate the inline object of a `Create` or `Update`. `None` means ignore it.
pub(crate) fn parse_contribution(activity: &Value) -> Option<Contribution> {
    let actor = activity.get("actor").and_then(Value::as_str)?;
    let object = activity.get("object")?;
    let kind = object.get("type").and_then(Value::as_str)?;
    if !matches!(kind, "Page" | "Note" | "Article") {
        return None;
    }
    let object_uri = object.get("id").and_then(Value::as_str)?;
    // A signer can only speak for itself and for objects on its own server.
    if object.get("attributedTo").and_then(Value::as_str) != Some(actor)
        || https_host(object_uri).is_none()
        || https_host(object_uri) != https_host(actor)
        || !is_public(object)
    {
        return None;
    }
    let text = object_text(object);
    let text = strip_leading_mentions(&text);
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .map(html_to_text)
        .filter(|n| !n.is_empty());
    let first_line = text.lines().find(|l| !l.trim().is_empty()).map(str::trim);
    let title = name.as_deref().or(first_line)?;
    let title = truncate_chars(title, MAX_TITLE_LEN);
    let body = if text.is_empty() {
        title.as_str()
    } else {
        text
    };
    Some(Contribution {
        object_uri: object_uri.to_owned(),
        body: body.to_owned(),
        in_reply_to: object
            .get("inReplyTo")
            .and_then(Value::as_str)
            .map(str::to_owned),
        title,
    })
}

/// `@name@host`, from a username the remote server controls.
fn remote_handle(preferred: Option<&str>, actor: &str) -> String {
    let host = https_host(actor).unwrap_or_default();
    let name: String = preferred
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .take(64)
        .collect();
    let name = if name.is_empty() { "user" } else { &name };
    format!("@{name}@{host}")
}

/// A forum object an activity refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    Topic(Uuid),
    Comment { id: Uuid, topic: Uuid },
}

/// Resolve an object URI, local or remote, to visible content of this forum.
async fn resolve_target(
    db: &PgPool,
    forum: &FedForum,
    group_actor: &str,
    uri: &str,
) -> Option<Target> {
    let local_id = uri
        .strip_prefix(group_actor)
        .and_then(|rest| rest.strip_prefix("/objects/"))
        .and_then(|id| Uuid::parse_str(id).ok());
    let topic: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM forum_topic
          WHERE forum_id = $1 AND hidden_at IS NULL
            AND ((id = $2 AND remote_actor_url IS NULL) OR ap_object_uri = $3)",
    )
    .bind(forum.id)
    .bind(local_id)
    .bind(uri)
    .fetch_optional(db)
    .await
    .ok()?;
    if let Some(topic) = topic {
        return Some(Target::Topic(topic));
    }
    let comment: (Uuid, Uuid) = sqlx::query_as(
        "SELECT c.id, c.topic_id FROM forum_topic_comment c
           JOIN forum_topic t ON t.id = c.topic_id
          WHERE t.forum_id = $1 AND t.hidden_at IS NULL
            AND c.hidden_at IS NULL AND c.moderation = 'approved'
            AND ((c.id = $2 AND NOT c.federated) OR c.ap_object_uri = $3)",
    )
    .bind(forum.id)
    .bind(local_id)
    .bind(uri)
    .fetch_optional(db)
    .await
    .ok()??;
    Some(Target::Comment {
        id: comment.0,
        topic: comment.1,
    })
}

/// The citizen remote topics hang from: no credentials and a subject no provider issues.
async fn system_author(db: &PgPool, org: Uuid) -> Result<Uuid, sqlx::Error> {
    sqlx::query(
        "INSERT INTO citizen (id, org_id, oidc_subject, verification_level, display_name, is_public, created_at)
         SELECT $1, $2, $3, 'anonymous', 'Fediverso', false, now()
          WHERE NOT EXISTS (SELECT 1 FROM citizen WHERE org_id = $2 AND oidc_subject = $3)",
    )
    .bind(Uuid::now_v7())
    .bind(org)
    .bind(SYSTEM_SUBJECT)
    .execute(db)
    .await?;
    sqlx::query_scalar("SELECT id FROM citizen WHERE org_id = $1 AND oidc_subject = $2")
        .bind(org)
        .bind(SYSTEM_SUBJECT)
        .fetch_one(db)
        .await
}

/// Queue an `Announce` of `inner` for the forum's followers, once per `dedupe_key`.
pub(crate) async fn enqueue_announce(
    db: &PgPool,
    forum_id: Uuid,
    group_actor: &str,
    inner: Value,
    dedupe_key: &str,
) -> Result<(), sqlx::Error> {
    let id = Uuid::now_v7();
    let announce = json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "id": format!("{group_actor}/activities/announce-{id}"),
        "type": "Announce",
        "actor": group_actor,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "cc": [format!("{group_actor}/followers")],
        "audience": group_actor,
        "object": inner,
    });
    sqlx::query(
        "INSERT INTO forum_outbound_activity (id, forum_id, dedupe_key, activity)
         VALUES ($1, $2, $3, $4) ON CONFLICT (dedupe_key) DO NOTHING",
    )
    .bind(id)
    .bind(forum_id)
    .bind(dedupe_key)
    .bind(announce)
    .execute(db)
    .await?;
    Ok(())
}

/// Repeat an accepted activity. A new thread also goes as a plain `Announce` of the object:
/// Mastodon and Pleroma cannot read `Announce(Create(Page))`, and Lemmy sends both for that reason.
pub(crate) async fn announce_accepted(
    db: &PgPool,
    forum_id: Uuid,
    group_actor: &str,
    activity: &Value,
    new_thread_object: Option<&str>,
) -> Result<(), sqlx::Error> {
    let activity_id = activity.get("id").and_then(Value::as_str).unwrap_or("");
    enqueue_announce(
        db,
        forum_id,
        group_actor,
        activity.clone(),
        &format!("relay:{forum_id}:{activity_id}"),
    )
    .await?;
    if let Some(object) = new_thread_object {
        enqueue_announce(
            db,
            forum_id,
            group_actor,
            json!(object),
            &format!("relay-plain:{forum_id}:{object}"),
        )
        .await?;
    }
    Ok(())
}

async fn refresh_for(db: &PgPool, target: Target) -> Result<(), sqlx::Error> {
    let topic = match target {
        Target::Topic(topic) => topic,
        Target::Comment { id, topic } => {
            dsoc_forums::queries::refresh_comment_counters(db, id).await?;
            topic
        }
    };
    dsoc_forums::queries::refresh_topic_counters(db, topic).await?;
    Ok(())
}

/// Everything an activity handler needs about who sent it and where it landed.
struct Ctx<'a> {
    db: &'a PgPool,
    forum: &'a FedForum,
    group_actor: String,
    actor: &'a str,
    activity: &'a Value,
    direct: bool,
    preferred_username: Option<&'a str>,
}

/// Whether the actor may skip the queue: a trusted host that is not under a domain silence.
async fn enters_directly(db: &PgPool, config: &InboundConfig, actor: &str) -> bool {
    let Some(host) = https_host(actor) else {
        return false;
    };
    if !config.trusted_hosts.contains(&host) {
        return false;
    }
    let silenced: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM server_domain_block WHERE domain = $1)")
            .bind(&host)
            .fetch_one(db)
            .await
            .unwrap_or(true);
    !silenced
}

/// Whether this account or its whole server is blocked.
pub(crate) async fn is_blocked(db: &PgPool, actor: &str) -> bool {
    let host = https_host(actor).unwrap_or_default();
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM forum_remote_actor_block WHERE actor_url = $1)
             OR EXISTS (SELECT 1 FROM server_domain_block WHERE domain = $2 AND severity = 'suspend')",
    )
    .bind(actor)
    .bind(host)
    .fetch_one(db)
    .await
    .unwrap_or(true)
}

/// Apply one signed activity to a forum.
///
/// # Errors
/// Propagates storage failures, so the sender can retry.
pub async fn apply(
    db: &PgPool,
    config: &InboundConfig,
    host: &str,
    handle: &str,
    activity: &Value,
    preferred_username: Option<&str>,
) -> Result<StatusCode, sqlx::Error> {
    let Some(forum) = lookup_by_handle(db, handle).await else {
        return Ok(StatusCode::NOT_FOUND);
    };
    if !config.enabled {
        return Ok(StatusCode::ACCEPTED);
    }
    let Some(actor) = activity.get("actor").and_then(Value::as_str) else {
        return Ok(StatusCode::BAD_REQUEST);
    };
    if https_host(actor).is_none_or(|h| h == host) {
        return Ok(StatusCode::ACCEPTED);
    }
    if is_blocked(db, actor).await {
        return Ok(StatusCode::FORBIDDEN);
    }
    let ctx = Ctx {
        db,
        forum: &forum,
        group_actor: format!("https://{host}/actors/{handle}"),
        actor,
        activity,
        direct: enters_directly(db, config, actor).await,
        preferred_username,
    };
    match activity.get("type").and_then(Value::as_str).unwrap_or("") {
        "Create" => on_create(&ctx).await?,
        "Update" => on_update(&ctx).await?,
        "Delete" => on_delete(&ctx).await?,
        "Like" => on_vote(&ctx, "favor").await?,
        "Dislike" => on_vote(&ctx, "contra").await?,
        "Undo" => on_undo(&ctx).await?,
        _ => {}
    }
    Ok(StatusCode::ACCEPTED)
}

async fn on_create(ctx: &Ctx<'_>) -> Result<(), sqlx::Error> {
    let Some(item) = parse_contribution(ctx.activity) else {
        return Ok(());
    };
    match item.in_reply_to.as_deref() {
        Some(parent) => create_comment(ctx, &item, parent).await,
        None => create_topic(ctx, &item).await,
    }
}

async fn create_topic(ctx: &Ctx<'_>, item: &Contribution) -> Result<(), sqlx::Error> {
    let object = ctx.activity.get("object").unwrap_or(&Value::Null);
    if !addressed_to(object, &ctx.group_actor) && !addressed_to(ctx.activity, &ctx.group_actor) {
        return Ok(());
    }
    if item.body.chars().count() > MAX_BODY_LEN {
        return Ok(());
    }
    let org: Uuid = sqlx::query_scalar("SELECT org_id FROM forum WHERE id = $1")
        .bind(ctx.forum.id)
        .fetch_one(ctx.db)
        .await?;
    let author = system_author(ctx.db, org).await?;
    let inserted = sqlx::query(
        "INSERT INTO forum_topic (id, forum_id, author_id, title, body, created_at, ap_object_uri,
                                  remote_actor_url, remote_handle, remote_activity, moderation, hidden_at)
         VALUES ($1, $2, $3, $4, $5, now(), $6, $7, $8, $9, $10,
                 CASE WHEN $11 THEN NULL ELSE now() END)
         ON CONFLICT (ap_object_uri) WHERE ap_object_uri IS NOT NULL DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(ctx.forum.id)
    .bind(author)
    .bind(&item.title)
    .bind(&item.body)
    .bind(&item.object_uri)
    .bind(ctx.actor)
    .bind(remote_handle(ctx.preferred_username, ctx.actor))
    .bind(ctx.activity)
    .bind(if ctx.direct { "approved" } else { "pending" })
    .bind(ctx.direct)
    .execute(ctx.db)
    .await?
    .rows_affected();
    if inserted > 0 && ctx.direct {
        announce_accepted(
            ctx.db,
            ctx.forum.id,
            &ctx.group_actor,
            ctx.activity,
            Some(&item.object_uri),
        )
        .await?;
    }
    Ok(())
}

async fn create_comment(
    ctx: &Ctx<'_>,
    item: &Contribution,
    parent: &str,
) -> Result<(), sqlx::Error> {
    let Some(target) = resolve_target(ctx.db, ctx.forum, &ctx.group_actor, parent).await else {
        return Ok(());
    };
    if item.body.chars().count() > MAX_COMMENT_LEN {
        return Ok(());
    }
    let (Target::Topic(topic) | Target::Comment { topic, .. }) = target;
    let inserted = sqlx::query(
        "INSERT INTO forum_topic_comment (id, topic_id, remote_actor_url, remote_handle, federated,
                                          moderation, body, created_at, ap_object_uri, remote_activity)
         VALUES ($1, $2, $3, $4, true, $5, $6, now(), $7, $8)
         ON CONFLICT (ap_object_uri) WHERE ap_object_uri IS NOT NULL DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(topic)
    .bind(ctx.actor)
    .bind(remote_handle(ctx.preferred_username, ctx.actor))
    .bind(if ctx.direct { "approved" } else { "pending" })
    .bind(&item.body)
    .bind(&item.object_uri)
    .bind(ctx.activity)
    .execute(ctx.db)
    .await?
    .rows_affected();
    if inserted > 0 && ctx.direct {
        dsoc_forums::queries::refresh_topic_counters(ctx.db, topic).await?;
        announce_accepted(ctx.db, ctx.forum.id, &ctx.group_actor, ctx.activity, None).await?;
    }
    Ok(())
}

/// An edit from an untrusted account goes back to the queue, or approval would let it swap
/// approved text for anything afterwards.
async fn on_update(ctx: &Ctx<'_>) -> Result<(), sqlx::Error> {
    let Some(item) = parse_contribution(ctx.activity) else {
        return Ok(());
    };
    if item.body.chars().count() > MAX_BODY_LEN {
        return Ok(());
    }
    let topic: Option<Uuid> = sqlx::query_scalar(
        "UPDATE forum_topic
            SET title = $3, body = $4, remote_activity = $5,
                moderation = CASE WHEN $6 THEN moderation ELSE 'pending' END,
                hidden_at = CASE WHEN $6 THEN hidden_at ELSE now() END
          WHERE ap_object_uri = $1 AND remote_actor_url = $2 AND forum_id = $7
            AND moderation <> 'rejected'
         RETURNING id",
    )
    .bind(&item.object_uri)
    .bind(ctx.actor)
    .bind(&item.title)
    .bind(&item.body)
    .bind(ctx.activity)
    .bind(ctx.direct)
    .bind(ctx.forum.id)
    .fetch_optional(ctx.db)
    .await?;
    let comment_topic: Option<Uuid> = if topic.is_some() {
        None
    } else if item.body.chars().count() > MAX_COMMENT_LEN {
        return Ok(());
    } else {
        sqlx::query_scalar(
            "UPDATE forum_topic_comment c
                SET body = $3, remote_activity = $4,
                    moderation = CASE WHEN $5 THEN c.moderation ELSE 'pending' END
               FROM forum_topic t
              WHERE t.id = c.topic_id AND t.forum_id = $6
                AND c.ap_object_uri = $1 AND c.remote_actor_url = $2
                AND c.moderation <> 'rejected'
             RETURNING c.topic_id",
        )
        .bind(&item.object_uri)
        .bind(ctx.actor)
        .bind(&item.body)
        .bind(ctx.activity)
        .bind(ctx.direct)
        .bind(ctx.forum.id)
        .fetch_optional(ctx.db)
        .await?
    };
    if let Some(topic) = comment_topic {
        dsoc_forums::queries::refresh_topic_counters(ctx.db, topic).await?;
    }
    if ctx.direct && (topic.is_some() || comment_topic.is_some()) {
        announce_accepted(ctx.db, ctx.forum.id, &ctx.group_actor, ctx.activity, None).await?;
    }
    Ok(())
}

async fn on_delete(ctx: &Ctx<'_>) -> Result<(), sqlx::Error> {
    let object = ctx.activity.get("object").unwrap_or(&Value::Null);
    let Some(uri) = object
        .as_str()
        .or_else(|| object.get("id").and_then(Value::as_str))
    else {
        return Ok(());
    };
    if uri == ctx.actor {
        return erase_actor(ctx.db, ctx.actor).await;
    }
    let was_public = hide_own_object(ctx, uri).await?;
    if was_public {
        announce_accepted(ctx.db, ctx.forum.id, &ctx.group_actor, ctx.activity, None).await?;
    }
    Ok(())
}

async fn hide_own_object(ctx: &Ctx<'_>, uri: &str) -> Result<bool, sqlx::Error> {
    let topic: Option<bool> = sqlx::query_scalar(
        "UPDATE forum_topic new SET moderation = 'rejected', hidden_at = now(),
                                    deletion_reason = 'deleted by its remote author'
           FROM forum_topic old
          WHERE new.id = old.id AND new.ap_object_uri = $1 AND new.remote_actor_url = $2
            AND new.forum_id = $3
         RETURNING old.hidden_at IS NULL",
    )
    .bind(uri)
    .bind(ctx.actor)
    .bind(ctx.forum.id)
    .fetch_optional(ctx.db)
    .await?;
    if let Some(was_public) = topic {
        return Ok(was_public);
    }
    let comment: Option<(Uuid, bool)> = sqlx::query_as(
        "UPDATE forum_topic_comment new
            SET moderation = 'rejected', hidden_at = now(),
                deletion_reason = 'deleted by its remote author'
           FROM forum_topic_comment old, forum_topic t
          WHERE new.id = old.id AND t.id = new.topic_id AND t.forum_id = $3
            AND new.ap_object_uri = $1 AND new.remote_actor_url = $2
         RETURNING new.topic_id, old.moderation = 'approved' AND old.hidden_at IS NULL",
    )
    .bind(uri)
    .bind(ctx.actor)
    .bind(ctx.forum.id)
    .fetch_optional(ctx.db)
    .await?;
    let Some((topic, was_public)) = comment else {
        return Ok(false);
    };
    dsoc_forums::queries::refresh_topic_counters(ctx.db, topic).await?;
    Ok(was_public)
}

/// A deleted remote account takes its content and votes with it.
async fn erase_actor(db: &PgPool, actor: &str) -> Result<(), sqlx::Error> {
    let mut topics: Vec<Uuid> = sqlx::query_scalar(
        "UPDATE forum_topic_comment SET moderation = 'rejected', hidden_at = now(),
                                        deletion_reason = 'remote account deleted'
          WHERE remote_actor_url = $1 RETURNING topic_id",
    )
    .bind(actor)
    .fetch_all(db)
    .await?;
    sqlx::query(
        "UPDATE forum_topic SET moderation = 'rejected', hidden_at = now(),
                                deletion_reason = 'remote account deleted'
          WHERE remote_actor_url = $1",
    )
    .bind(actor)
    .execute(db)
    .await?;
    let voted: Vec<(String, Uuid)> = sqlx::query_as(
        "DELETE FROM forum_remote_vote WHERE actor_url = $1 RETURNING target_kind, target_id",
    )
    .bind(actor)
    .fetch_all(db)
    .await?;
    for (kind, id) in voted {
        if kind == "topic" {
            topics.push(id);
        } else {
            dsoc_forums::queries::refresh_comment_counters(db, id).await?;
            let topic: Option<Uuid> =
                sqlx::query_scalar("SELECT topic_id FROM forum_topic_comment WHERE id = $1")
                    .bind(id)
                    .fetch_optional(db)
                    .await?;
            topics.extend(topic);
        }
    }
    topics.sort_unstable();
    topics.dedup();
    for topic in topics {
        dsoc_forums::queries::refresh_topic_counters(db, topic).await?;
    }
    Ok(())
}

fn target_row(target: Target) -> (&'static str, Uuid) {
    match target {
        Target::Topic(id) => ("topic", id),
        Target::Comment { id, .. } => ("comment", id),
    }
}

async fn on_vote(ctx: &Ctx<'_>, stance: &str) -> Result<(), sqlx::Error> {
    let Some(uri) = ctx.activity.get("object").and_then(Value::as_str) else {
        return Ok(());
    };
    let Some(target) = resolve_target(ctx.db, ctx.forum, &ctx.group_actor, uri).await else {
        return Ok(());
    };
    let (kind, id) = target_row(target);
    sqlx::query(
        "INSERT INTO forum_remote_vote (target_kind, target_id, actor_url, stance, activity_id)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (target_kind, target_id, actor_url)
         DO UPDATE SET stance = EXCLUDED.stance, activity_id = EXCLUDED.activity_id",
    )
    .bind(kind)
    .bind(id)
    .bind(ctx.actor)
    .bind(stance)
    .bind(ctx.activity.get("id").and_then(Value::as_str))
    .execute(ctx.db)
    .await?;
    refresh_for(ctx.db, target).await?;
    announce_accepted(ctx.db, ctx.forum.id, &ctx.group_actor, ctx.activity, None).await
}

async fn on_undo(ctx: &Ctx<'_>) -> Result<(), sqlx::Error> {
    let inner = ctx.activity.get("object").unwrap_or(&Value::Null);
    let inner_type = inner.get("type").and_then(Value::as_str);
    if !matches!(inner_type, Some("Like" | "Dislike"))
        || inner.get("actor").and_then(Value::as_str) != Some(ctx.actor)
    {
        return Ok(());
    }
    let Some(uri) = inner.get("object").and_then(Value::as_str) else {
        return Ok(());
    };
    let Some(target) = resolve_target(ctx.db, ctx.forum, &ctx.group_actor, uri).await else {
        return Ok(());
    };
    let (kind, id) = target_row(target);
    let removed = sqlx::query(
        "DELETE FROM forum_remote_vote WHERE target_kind = $1 AND target_id = $2 AND actor_url = $3",
    )
    .bind(kind)
    .bind(id)
    .bind(ctx.actor)
    .execute(ctx.db)
    .await?
    .rows_affected();
    if removed > 0 {
        refresh_for(ctx.db, target).await?;
        announce_accepted(ctx.db, ctx.forum.id, &ctx.group_actor, ctx.activity, None).await?;
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn html_becomes_text_and_markup_does_not_survive() {
        assert_eq!(
            html_to_text("<p>um &amp; dois</p><p>tr<b>ês</b><br/>quatro</p>"),
            "um & dois\n\ntrês\nquatro"
        );
        let text =
            html_to_text("a<script>alert(1)</script>b<style>x{}</style>c<img src=x onerror=y>");
        assert_eq!(text, "abc");
        assert_eq!(html_to_text("x<unterminated"), "x");
        // Escaped markup stays text: rendering escapes it again.
        assert_eq!(html_to_text("&lt;script&gt;"), "<script>");
    }

    #[test]
    fn leading_mentions_are_addressing() {
        assert_eq!(strip_leading_mentions("@a @b@c.d Olá @e"), "Olá @e");
        assert_eq!(strip_leading_mentions("@a"), "");
    }

    fn lemmy_create() -> Value {
        // Shape from join-lemmy.org/docs/contributors/05-federation.html
        json!({
            "actor": "https://ds9.example/u/lemmy_alpha",
            "to": ["https://www.w3.org/ns/activitystreams#Public"],
            "object": {
                "type": "Page",
                "id": "https://ds9.example/post/1",
                "attributedTo": "https://ds9.example/u/lemmy_alpha",
                "to": ["https://agora.test/actors/boas-vindas", "https://www.w3.org/ns/activitystreams#Public"],
                "audience": "https://agora.test/actors/boas-vindas",
                "name": "test post",
                "content": "<p>test body</p>\n",
                "mediaType": "text/html",
                "source": {"content": "test **body**", "mediaType": "text/markdown"},
                "attachment": [{"type": "Link", "href": "https://ds9.example/pictrs/image/x.jpg"}],
                "sensitive": false,
                "published": "2021-10-29T15:10:51.557399Z"
            },
            "cc": ["https://agora.test/actors/boas-vindas"],
            "audience": "https://agora.test/actors/boas-vindas",
            "type": "Create",
            "id": "https://ds9.example/activities/create/eee6a57a"
        })
    }

    #[test]
    fn a_lemmy_post_keeps_title_markdown_and_link() {
        let item = parse_contribution(&lemmy_create()).unwrap();
        assert_eq!(item.title, "test post");
        assert_eq!(
            item.body,
            "test **body**\n\nhttps://ds9.example/pictrs/image/x.jpg"
        );
        assert_eq!(item.in_reply_to, None);
    }

    fn mastodon_mention(to: Value) -> Value {
        // Shape of a Mastodon post that mentions the forum.
        json!({
            "id": "https://m.example/users/ana/statuses/1/activity",
            "type": "Create",
            "actor": "https://m.example/users/ana",
            "to": to,
            "cc": ["https://agora.test/actors/boas-vindas"],
            "object": {
                "id": "https://m.example/users/ana/statuses/1",
                "type": "Note",
                "summary": null,
                "inReplyTo": null,
                "attributedTo": "https://m.example/users/ana",
                "to": to,
                "cc": ["https://m.example/users/ana/followers", "https://agora.test/actors/boas-vindas"],
                "content": "<p><span class=\"h-card\"><a href=\"https://agora.test/actors/boas-vindas\" class=\"u-url mention\">@<span>boas-vindas</span></a></span> Primeira linha</p><p>Resto do texto</p>",
                "tag": [{"type": "Mention", "href": "https://agora.test/actors/boas-vindas", "name": "@boas-vindas@agora.test"}]
            }
        })
    }

    #[test]
    fn a_mastodon_mention_takes_its_title_from_the_first_line() {
        let item = parse_contribution(&mastodon_mention(json!([
            "https://www.w3.org/ns/activitystreams#Public"
        ])))
        .unwrap();
        assert_eq!(item.title, "Primeira linha");
        assert_eq!(item.body, "Primeira linha\n\nResto do texto");
    }

    #[test]
    fn a_direct_message_is_never_a_contribution() {
        assert!(parse_contribution(&mastodon_mention(json!([
            "https://agora.test/actors/boas-vindas"
        ])))
        .is_none());
    }

    #[test]
    fn a_signer_cannot_speak_for_someone_else() {
        let mut other_author = lemmy_create();
        other_author["object"]["attributedTo"] = json!("https://other.example/u/x");
        assert!(parse_contribution(&other_author).is_none());
        let mut other_host = lemmy_create();
        other_host["object"]["id"] = json!("https://other.example/post/1");
        assert!(parse_contribution(&other_host).is_none());
        let mut insecure = lemmy_create();
        insecure["object"]["id"] = json!("http://ds9.example/post/1");
        assert!(parse_contribution(&insecure).is_none());
    }

    #[test]
    fn a_title_is_cut_to_the_forum_limit() {
        let mut long = lemmy_create();
        long["object"]["name"] = json!("x".repeat(MAX_TITLE_LEN + 50));
        assert_eq!(
            parse_contribution(&long).unwrap().title.chars().count(),
            MAX_TITLE_LEN
        );
    }

    #[test]
    fn a_remote_username_cannot_inject_markup() {
        assert_eq!(
            remote_handle(Some("<b>ana</b>"), "https://m.example/u/1"),
            "@banab@m.example"
        );
        assert_eq!(
            remote_handle(None, "https://m.example/u/1"),
            "@user@m.example"
        );
    }

    #[test]
    fn the_policy_is_off_by_default() {
        assert!(!InboundConfig::default().enabled);
    }
}

//! What a forum's Group actor publishes to its followers beyond its topics' `Announce`: local
//! comments, removals and the activities it repeats (FEP-1b12), plus the minimal actor document
//! that lets a remote server verify an author whose profile is private.

use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use crate::forum_federation::{ensure_forum_keys, path_to_handle};
use crate::forum_inbound::{enqueue_announce, InboundConfig};

const PUBLIC: &str = "https://www.w3.org/ns/activitystreams#Public";
const MAX_ATTEMPTS: i32 = 5;
const SEND_BATCH: i64 = 50;

/// The actor handle of a local author. A private profile gets an opaque handle, so federating a
/// post does not reveal the username the person chose to keep private.
#[must_use]
pub fn author_handle(citizen: Uuid, handle: Option<&str>, is_public: bool) -> String {
    match handle {
        Some(handle) if is_public => handle.to_owned(),
        _ => format!("u_{}", citizen.simple()),
    }
}

/// The citizen behind an opaque author handle, when it has that shape.
fn opaque_citizen(handle: &str) -> Option<Uuid> {
    let hex = handle.strip_prefix("u_")?;
    (hex.len() == 32)
        .then(|| Uuid::parse_str(hex).ok())
        .flatten()
}

/// The citizen an opaque handle may be served for: an active private author with public content
/// in a federated forum. Everyone else, including the same id with no such content, does not exist.
pub async fn resolve_opaque_author(db: &PgPool, handle: &str) -> Option<Uuid> {
    let citizen = opaque_citizen(handle)?;
    let eligible: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM citizen c
             WHERE c.id = $1 AND c.deleted_at IS NULL AND c.suspended_at IS NULL
               AND NOT (c.is_public AND c.handle IS NOT NULL)
               AND (EXISTS (SELECT 1 FROM forum_topic t JOIN forum f ON f.id = t.forum_id
                             WHERE t.author_id = c.id AND t.hidden_at IS NULL
                               AND t.remote_actor_url IS NULL
                               AND f.federated AND forum_readable(f.id, NULL))
                 OR EXISTS (SELECT 1 FROM forum_topic_comment m
                              JOIN forum_topic t ON t.id = m.topic_id
                              JOIN forum f ON f.id = t.forum_id
                             WHERE m.author_id = c.id AND m.hidden_at IS NULL
                               AND t.hidden_at IS NULL
                               AND f.federated AND forum_readable(f.id, NULL))))",
    )
    .bind(citizen)
    .fetch_one(db)
    .await
    .ok()?;
    eligible.then_some(citizen)
}

/// Enough of a `Person` for a remote server to verify authorship, and nothing about the person.
#[must_use]
pub fn opaque_author_json(host: &str, handle: &str, public_pem: &str) -> Value {
    let actor = format!("https://{host}/actors/{handle}");
    json!({
        "@context": ["https://www.w3.org/ns/activitystreams", "https://w3id.org/security/v1"],
        "id": actor,
        "type": "Person",
        "preferredUsername": handle,
        "inbox": format!("{actor}/inbox"),
        "outbox": format!("{actor}/outbox"),
        "manuallyApprovesFollowers": true,
        "discoverable": false,
        "indexable": false,
        "publicKey": {
            "id": format!("{actor}#main-key"),
            "owner": actor,
            "publicKeyPem": public_pem,
        },
    })
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Forum text as the HTML `content` of an object, with the markdown original in `source`.
pub(crate) fn content_fields(body: &str, title: Option<&str>) -> (String, Value) {
    let text = escape_html(body).replace('\n', "<br/>");
    let content = match title {
        Some(title) => format!(
            "<p><strong>{}</strong></p><p>{text}</p>",
            escape_html(title)
        ),
        None => format!("<p>{text}</p>"),
    };
    (
        content,
        json!({ "content": body, "mediaType": "text/markdown" }),
    )
}

/// A `Create` of a local object, attributed to its author and repeated by the Group.
pub(crate) fn create_activity(group_actor: &str, object: &Value, activity_key: &str) -> Value {
    json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "id": format!("{group_actor}/activities/create-{activity_key}"),
        "type": "Create",
        "actor": object.get("attributedTo").cloned().unwrap_or(Value::Null),
        "to": [PUBLIC],
        "cc": [group_actor, format!("{group_actor}/followers")],
        "audience": group_actor,
        "object": object,
    })
}

/// A local comment as a dereferenceable `Note`.
pub async fn comment_object_json(
    db: &PgPool,
    host: &str,
    handle: &str,
    comment: Uuid,
) -> Option<Value> {
    #[derive(sqlx::FromRow)]
    struct Row {
        body: String,
        created_at: chrono::DateTime<chrono::Utc>,
        topic_id: Uuid,
        topic_uri: Option<String>,
        author_id: Uuid,
        author_handle: Option<String>,
        is_public: bool,
    }
    let path = crate::forum_federation::handle_to_path(handle)?;
    let row: Row = sqlx::query_as(
        "SELECT m.body, m.created_at, m.topic_id, t.ap_object_uri AS topic_uri,
                m.author_id, c.handle AS author_handle, c.is_public
           FROM forum_topic_comment m
           JOIN forum_topic t ON t.id = m.topic_id
           JOIN forum f ON f.id = t.forum_id
           JOIN citizen c ON c.id = m.author_id
          WHERE m.id = $1 AND NOT m.federated AND m.hidden_at IS NULL AND m.moderation = 'approved'
            AND t.hidden_at IS NULL AND f.full_path = $2
            AND f.federated AND forum_readable(f.id, NULL)",
    )
    .bind(comment)
    .bind(path)
    .fetch_optional(db)
    .await
    .ok()??;
    let group = format!("https://{host}/actors/{handle}");
    let author = author_handle(row.author_id, row.author_handle.as_deref(), row.is_public);
    let (content, source) = content_fields(&row.body, None);
    Some(json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "id": format!("{group}/objects/{comment}"),
        "type": "Note",
        "attributedTo": format!("https://{host}/actors/{author}"),
        "to": [PUBLIC],
        "cc": [group, format!("{group}/followers")],
        "audience": group,
        "inReplyTo": row.topic_uri.unwrap_or_else(|| format!("{group}/objects/{}", row.topic_id)),
        "content": content,
        "mediaType": "text/html",
        "source": source,
        "url": format!("https://{host}/f/topico/{}", row.topic_id),
        "published": row.created_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    }))
}

/// Queue local comments from forums with followers. Only with inbound on: an instance that has not
/// opted in keeps federating topics alone.
async fn enqueue_local_comments(db: &PgPool, host: &str) -> Result<(), sqlx::Error> {
    let pending: Vec<(Uuid, Uuid, String)> = sqlx::query_as(
        "SELECT m.id, f.id, f.full_path
           FROM forum_topic_comment m
           JOIN forum_topic t ON t.id = m.topic_id
           JOIN forum f ON f.id = t.forum_id
          WHERE NOT m.federated AND m.hidden_at IS NULL AND m.moderation = 'approved'
            AND t.hidden_at IS NULL AND m.created_at > now() - interval '7 days'
            AND f.federated AND forum_readable(f.id, NULL)
            AND EXISTS (SELECT 1 FROM forum_follower ff
                         WHERE ff.forum_id = f.id AND ff.accepted_at IS NOT NULL)
            AND NOT EXISTS (SELECT 1 FROM forum_outbound_activity a
                             WHERE a.dedupe_key = 'local-comment:' || m.id::text)
          LIMIT 100",
    )
    .fetch_all(db)
    .await?;
    for (comment, forum, path) in pending {
        let handle = path_to_handle(&path);
        let Some(note) = comment_object_json(db, host, &handle, comment).await else {
            continue;
        };
        let group = format!("https://{host}/actors/{handle}");
        let create = create_activity(&group, &note, &comment.to_string());
        enqueue_announce(
            db,
            forum,
            &group,
            create,
            &format!("local-comment:{comment}"),
        )
        .await?;
    }
    Ok(())
}

/// Tell followers that moderation removed something. A local author's post is deleted in the
/// author's name; for a remote author's post only the Group can speak, so the actor is the Group.
pub async fn enqueue_removal(
    db: &PgPool,
    host: &str,
    kind: &str,
    id: Uuid,
    reason: Option<&str>,
) -> Result<(), sqlx::Error> {
    #[derive(sqlx::FromRow)]
    struct Row {
        forum_id: Uuid,
        full_path: String,
        object_uri: Option<String>,
        author_id: Option<Uuid>,
        author_handle: Option<String>,
        is_public: Option<bool>,
    }
    let select = if kind == "topic" {
        "SELECT f.id AS forum_id, f.full_path, t.ap_object_uri AS object_uri,
                CASE WHEN t.remote_actor_url IS NULL THEN t.author_id END AS author_id,
                c.handle AS author_handle, c.is_public
           FROM forum_topic t JOIN forum f ON f.id = t.forum_id
           JOIN citizen c ON c.id = t.author_id
          WHERE t.id = $1 AND f.federated AND forum_readable(f.id, NULL)"
    } else {
        "SELECT f.id AS forum_id, f.full_path, m.ap_object_uri AS object_uri,
                m.author_id, c.handle AS author_handle, c.is_public
           FROM forum_topic_comment m JOIN forum_topic t ON t.id = m.topic_id
           JOIN forum f ON f.id = t.forum_id
           LEFT JOIN citizen c ON c.id = m.author_id
          WHERE m.id = $1 AND f.federated AND forum_readable(f.id, NULL)"
    };
    let Some(row): Option<Row> = sqlx::query_as(select).bind(id).fetch_optional(db).await? else {
        return Ok(());
    };
    let handle = path_to_handle(&row.full_path);
    let group = format!("https://{host}/actors/{handle}");
    let actor = match row.author_id {
        Some(author) => format!(
            "https://{host}/actors/{}",
            author_handle(
                author,
                row.author_handle.as_deref(),
                row.is_public.unwrap_or(false)
            )
        ),
        None => group.clone(),
    };
    let object = row
        .object_uri
        .unwrap_or_else(|| format!("{group}/objects/{id}"));
    let delete = json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "id": format!("{group}/activities/delete-{id}"),
        "type": "Delete",
        "actor": actor,
        "to": [PUBLIC],
        "cc": [group],
        "audience": group,
        "object": object,
        "summary": reason,
    });
    enqueue_announce(db, row.forum_id, &group, delete, &format!("removal:{id}")).await
}

/// One worker tick: queue, fan out and send. Restricted forums are re-checked at send time, so a
/// forum that stopped being public sends nothing more.
pub async fn sweep(db: &PgPool, host: &str, config: &InboundConfig) {
    if config.enabled {
        if let Err(err) = enqueue_local_comments(db, host).await {
            tracing::warn!(?err, "forum outbound: queueing comments failed");
        }
    }
    let fanned = sqlx::query(
        "INSERT INTO forum_outbound_delivery (activity_id, recipient_inbox)
         SELECT DISTINCT a.id, ff.remote_inbox_url
           FROM forum_outbound_activity a
           JOIN forum_follower ff ON ff.forum_id = a.forum_id AND ff.accepted_at IS NOT NULL
          WHERE a.created_at > now() - interval '7 days' AND ff.created_at <= a.created_at
         ON CONFLICT DO NOTHING",
    )
    .execute(db)
    .await;
    if let Err(err) = fanned {
        tracing::warn!(?err, "forum outbound: fan-out failed");
        return;
    }
    let pending: Result<Vec<(Uuid, Uuid, String, Value)>, _> = sqlx::query_as(
        "SELECT d.activity_id, a.forum_id, d.recipient_inbox, a.activity
           FROM forum_outbound_delivery d
           JOIN forum_outbound_activity a ON a.id = d.activity_id
           JOIN forum f ON f.id = a.forum_id
          WHERE d.sent_at IS NULL AND d.attempts < $1
            AND f.federated AND forum_readable(f.id, NULL)
          ORDER BY a.created_at LIMIT $2",
    )
    .bind(MAX_ATTEMPTS)
    .bind(SEND_BATCH)
    .fetch_all(db)
    .await;
    let pending = match pending {
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!(?err, "forum outbound: lookup failed");
            return;
        }
    };
    for (activity_id, forum_id, inbox, activity) in pending {
        let Some((_, private_pem)) = ensure_forum_keys(db, forum_id).await else {
            continue;
        };
        let actor = activity.get("actor").and_then(Value::as_str).unwrap_or("");
        let sent = crate::federation::deliver_signed(actor, &private_pem, &inbox, &activity).await;
        let update = if let Err(err) = sent {
            tracing::warn!(?err, inbox = %inbox, "forum outbound: delivery failed");
            "UPDATE forum_outbound_delivery SET attempts = attempts + 1
              WHERE activity_id = $1 AND recipient_inbox = $2"
        } else {
            "UPDATE forum_outbound_delivery SET sent_at = now()
              WHERE activity_id = $1 AND recipient_inbox = $2"
        };
        if let Err(err) = sqlx::query(update)
            .bind(activity_id)
            .bind(&inbox)
            .execute(db)
            .await
        {
            tracing::warn!(?err, "forum outbound: recording delivery failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_private_author_gets_an_opaque_handle() {
        let id = Uuid::now_v7();
        assert_eq!(author_handle(id, Some("ana"), true), "ana");
        assert_eq!(
            author_handle(id, Some("ana"), false),
            format!("u_{}", id.simple())
        );
        assert_eq!(author_handle(id, None, true), format!("u_{}", id.simple()));
        assert_eq!(opaque_citizen(&format!("u_{}", id.simple())), Some(id));
        assert_eq!(opaque_citizen("u_short"), None);
        assert_eq!(opaque_citizen("ana"), None);
    }

    #[test]
    fn the_opaque_actor_says_nothing_about_the_person() {
        let doc = opaque_author_json("agora.test", "u_abc", "PEM");
        for field in ["name", "summary", "icon", "image", "url", "published"] {
            assert!(doc.get(field).is_none(), "{field}");
        }
        assert_eq!(doc["type"], "Person");
    }

    #[test]
    fn text_is_escaped_in_content_and_kept_in_source() {
        let (content, source) = content_fields("<b>x</b>", Some("t&t"));
        assert_eq!(
            content,
            "<p><strong>t&amp;t</strong></p><p>&lt;b&gt;x&lt;/b&gt;</p>"
        );
        assert_eq!(source["content"], "<b>x</b>");
    }
}

//! # F4 — forums as Fediverse `Group` actors (FEP-1b12, outbound slice).
//!
//! Handles carry NO prefix (decision 2026-07-26): path segments REVERSED and
//! joined by dots — `@ministerio-educacao@`, `@sp@`, `@saude.sp@` (department),
//! `@santos.sp@` (city), `@saude.santos.sp@` (municipal section), `@ccj.senado@`.
//! Resolution ALWAYS tries a citizen first (compat); forums are the fallback.
//!
//! Flows: WebFinger/Group actor (federation.rs delegates here) · Follow→Accept
//! signed with the forum's key (lazy generation) · Announce of new topics to
//! followers (fan-out swept by the worker, once per inbox) · dereferenceable
//! topic Page at `/actors/<handle>/objects/<topic_id>`. Everything else a remote
//! server sends is in [`crate::forum_inbound`].

use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

/// `saude.santos.sp` → `sp/santos/saude`. `None` when it does not look like a forum handle.
pub(crate) fn handle_to_path(handle: &str) -> Option<String> {
    if handle.is_empty() || handle.len() > 120 {
        return None;
    }
    let mut segs: Vec<&str> = handle.split('.').collect();
    if segs.len() > 3
        || segs.iter().any(|s| {
            s.is_empty()
                || !s
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
    {
        return None;
    }
    segs.reverse();
    Some(segs.join("/"))
}

/// `sp/santos/saude` → `saude.santos.sp`.
pub(crate) fn path_to_handle(path: &str) -> String {
    let mut segs: Vec<&str> = path.split('/').collect();
    segs.reverse();
    segs.join(".")
}

/// A forum resolved for federation.
#[derive(sqlx::FromRow, Debug)]
pub(crate) struct FedForum {
    pub(crate) id: Uuid,
    pub(crate) full_path: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) federated: bool,
    pub(crate) avatar_url: Option<String>,
    pub(crate) banner_url: Option<String>,
}

/// Resolve a federated handle to a visible forum (federated + not hidden).
pub(crate) async fn lookup_by_handle(db: &PgPool, handle: &str) -> Option<FedForum> {
    let path = handle_to_path(handle)?;
    sqlx::query_as(
        r"SELECT id, full_path, name, description, federated, avatar_url, banner_url
          FROM forum WHERE full_path = $1 AND federated AND forum_readable(id, NULL)",
    )
    .bind(&path)
    .fetch_optional(db)
    .await
    .ok()
    .flatten()
    .filter(|f: &FedForum| f.federated)
}

/// Group keypair — lazy generation, race-safe (guarded UPDATE + re-select).
pub(crate) async fn ensure_forum_keys(db: &PgPool, forum_id: Uuid) -> Option<(String, String)> {
    let row: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT public_key_pem, private_key_pem FROM forum WHERE id = $1")
            .bind(forum_id)
            .fetch_optional(db)
            .await
            .ok()?;
    match row {
        Some((Some(public), Some(private))) => Some((public, private)),
        Some(_) => {
            let kp = tokio::task::spawn_blocking(dsoc_federation::generate_actor_keypair)
                .await
                .ok()?
                .ok()?;
            let _ = sqlx::query(
                "UPDATE forum SET public_key_pem = $2, private_key_pem = $3
                 WHERE id = $1 AND private_key_pem IS NULL",
            )
            .bind(forum_id)
            .bind(&kp.public_pem)
            .bind(&kp.private_pem)
            .execute(db)
            .await;
            let row: Option<(Option<String>, Option<String>)> =
                sqlx::query_as("SELECT public_key_pem, private_key_pem FROM forum WHERE id = $1")
                    .bind(forum_id)
                    .fetch_optional(db)
                    .await
                    .ok()?;
            match row {
                Some((Some(pu), Some(pr))) => Some((pu, pr)),
                _ => None,
            }
        }
        None => None,
    }
}

/// WebFinger JRD for a forum handle.
pub(crate) fn webfinger_jrd(host: &str, handle: &str, resource: &str) -> Value {
    json!({
        "subject": resource,
        "links": [{
            "rel": "self",
            "type": "application/activity+json",
            "href": format!("https://{host}/actors/{handle}")
        }]
    })
}

/// The `Group` actor document.
pub(crate) fn group_actor_json(
    host: &str,
    handle: &str,
    forum: &FedForum,
    public_pem: &str,
) -> Value {
    let actor = format!("https://{host}/actors/{handle}");
    let mut doc = json!({
        "@context": ["https://www.w3.org/ns/activitystreams", "https://w3id.org/security/v1"],
        "id": actor,
        "type": "Group",
        "preferredUsername": handle,
        "name": forum.name,
        // User-facing copy: stays in the installation's locale until the i18n
        // catalog lands (wave 2.5); the description itself is author-provided.
        "summary": if forum.description.is_empty() {
            format!("Fórum institucional /f/{} na DemocraciaBR.", forum.full_path)
        } else {
            forum.description.clone()
        },
        "url": format!("https://{host}/f/{}", forum.full_path),
        "inbox": format!("{actor}/inbox"),
        "outbox": format!("{actor}/outbox"),
        "followers": format!("{actor}/followers"),
        "manuallyApprovesFollowers": false,
        // Tells Lemmy not to offer posting where this instance does not accept contributions.
        "postingRestrictedToMods": !crate::forum_inbound::InboundConfig::from_env().enabled,
        "publicKey": {
            "id": format!("{actor}#main-key"),
            "owner": actor,
            "publicKeyPem": public_pem,
        },
    });
    if let Some(url) = &forum.avatar_url {
        doc["icon"] = json!({ "type": "Image", "url": absolutize_url(host, url) });
    }
    if let Some(url) = &forum.banner_url {
        doc["image"] = json!({ "type": "Image", "url": absolutize_url(host, url) });
    }
    doc
}

/// `/media/x.png` → `https://<host>/media/x.png`; absolute URLs pass through untouched.
fn absolutize_url(host: &str, url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        url.to_owned()
    } else {
        format!("https://{}/{}", host, url.trim_start_matches('/'))
    }
}

/// `Follow` or `Undo{Follow}`, the only activities a forum takes with inbound off.
pub(crate) fn is_follow_activity(activity: &Value) -> bool {
    match activity.get("type").and_then(Value::as_str) {
        Some("Follow") => true,
        Some("Undo") => activity.pointer("/object/type").and_then(Value::as_str) == Some("Follow"),
        _ => false,
    }
}

/// Group inbox: `Follow` → record the follower (the REAL inbox comes from the
/// actor doc, not the payload — limits spoofing) and reply with an `Accept`
/// signed by the forum key; `Undo{Follow}` → remove. The rest goes to
/// [`crate::forum_inbound`], which ignores it unless the instance opted in.
pub(crate) async fn inbox(
    db: &PgPool,
    config: &crate::forum_inbound::InboundConfig,
    host: &str,
    handle: &str,
    forum: &FedForum,
    activity: &Value,
    preferred_username: Option<&str>,
) -> axum::http::StatusCode {
    use axum::http::StatusCode;
    let kind = activity.get("type").and_then(Value::as_str).unwrap_or("");
    let actor_url = activity.get("actor").and_then(Value::as_str).unwrap_or("");
    if actor_url.is_empty() {
        return StatusCode::BAD_REQUEST;
    }
    let our_actor = format!("https://{host}/actors/{handle}");

    match kind {
        "Follow" => {
            // Fetch the remote actor (instance-signed fetch) and use the DECLARED inbox.
            let Ok(doc) = crate::federation::fetch_remote_actor(actor_url).await else {
                tracing::warn!(actor_url, "forum follow: remote actor did not resolve");
                return StatusCode::ACCEPTED;
            };
            let inbox_url = doc
                .get("endpoints")
                .and_then(|e| e.get("sharedInbox"))
                .and_then(Value::as_str)
                .or_else(|| doc.get("inbox").and_then(Value::as_str))
                .unwrap_or_default()
                .to_owned();
            if !inbox_url.starts_with("https://") {
                return StatusCode::ACCEPTED;
            }
            let _ = sqlx::query(
                "INSERT INTO forum_follower (forum_id, remote_actor_url, remote_inbox_url, accepted_at)
                 VALUES ($1, $2, $3, now())
                 ON CONFLICT (forum_id, remote_actor_url)
                 DO UPDATE SET remote_inbox_url = EXCLUDED.remote_inbox_url, accepted_at = now()",
            )
            .bind(forum.id)
            .bind(actor_url)
            .bind(&inbox_url)
            .execute(db)
            .await;
            // Accept signed with the Group key — without it Mastodon shows "pending".
            if let Some((_, private_pem)) = ensure_forum_keys(db, forum.id).await {
                let accept = json!({
                    "@context": "https://www.w3.org/ns/activitystreams",
                    "id": format!("{our_actor}/activities/accept-{}", Uuid::now_v7()),
                    "type": "Accept",
                    "actor": our_actor,
                    "object": activity,
                });
                let our_actor2 = our_actor.clone();
                let inbox2 = inbox_url.clone();
                tokio::spawn(async move {
                    if let Err(err) = crate::federation::deliver_signed(
                        &our_actor2,
                        &private_pem,
                        &inbox2,
                        &accept,
                    )
                    .await
                    {
                        tracing::warn!(?err, inbox = %inbox2, "forum follow: Accept delivery failed");
                    }
                });
            }
            tracing::info!(forum = %forum.full_path, remote = %actor_url, "forum: new fediverse follower");
            StatusCode::ACCEPTED
        }
        "Undo" => {
            let is_follow = activity
                .get("object")
                .and_then(|o| o.get("type"))
                .and_then(Value::as_str)
                .map(|t| t == "Follow")
                .unwrap_or(false);
            if is_follow {
                let _ = sqlx::query(
                    "DELETE FROM forum_follower WHERE forum_id = $1 AND remote_actor_url = $2",
                )
                .bind(forum.id)
                .bind(actor_url)
                .execute(db)
                .await;
                return StatusCode::ACCEPTED;
            }
            apply_inbound(db, config, host, handle, activity, preferred_username).await
        }
        _ => apply_inbound(db, config, host, handle, activity, preferred_username).await,
    }
}

async fn apply_inbound(
    db: &PgPool,
    config: &crate::forum_inbound::InboundConfig,
    host: &str,
    handle: &str,
    activity: &Value,
    preferred_username: Option<&str>,
) -> axum::http::StatusCode {
    match crate::forum_inbound::apply(db, config, host, handle, activity, preferred_username).await
    {
        Ok(status) => status,
        Err(err) => {
            tracing::warn!(?err, "forum inbox: applying the activity failed");
            // The activity was already marked as seen, so make the retry count.
            let id = activity.get("id").and_then(Value::as_str).unwrap_or("");
            let _ = sqlx::query("DELETE FROM forum_inbox_seen WHERE activity_id = $1")
                .bind(id)
                .execute(db)
                .await;
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

/// Dereferenceable AP `Page` for a local topic (`/actors/<handle>/objects/<topic_id>`). Lemmy
/// reads a thread as `Page`, and Mastodon converts it to a status with a link.
pub(crate) async fn topic_object_json(
    db: &PgPool,
    host: &str,
    handle: &str,
    topic_id: Uuid,
) -> Option<Value> {
    #[derive(sqlx::FromRow)]
    struct Row {
        title: String,
        body: String,
        created_at: chrono::DateTime<chrono::Utc>,
        author_id: Uuid,
        author_handle: Option<String>,
        is_public: bool,
    }
    let row: Row = sqlx::query_as(
        r"SELECT t.title, t.body, t.created_at, t.author_id, c.handle AS author_handle, c.is_public
              FROM forum_topic t
              JOIN forum f ON f.id = t.forum_id
              JOIN citizen c ON c.id = t.author_id
              WHERE t.id = $1 AND t.hidden_at IS NULL AND t.remote_actor_url IS NULL
                AND f.full_path = $2 AND f.federated AND forum_readable(f.id, NULL)",
    )
    .bind(topic_id)
    .bind(handle_to_path(handle)?)
    .fetch_optional(db)
    .await
    .ok()??;
    let actor = format!("https://{host}/actors/{handle}");
    let author = crate::forum_outbound::author_handle(
        row.author_id,
        row.author_handle.as_deref(),
        row.is_public,
    );
    let (content, source) = crate::forum_outbound::content_fields(&row.body, Some(&row.title));
    Some(json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "id": format!("{actor}/objects/{topic_id}"),
        "type": "Page",
        "attributedTo": format!("https://{host}/actors/{author}"),
        "audience": actor,
        "to": [actor, "https://www.w3.org/ns/activitystreams#Public"],
        "cc": [format!("{actor}/followers")],
        "name": row.title,
        "content": content,
        "mediaType": "text/html",
        "source": source,
        "url": format!("https://{host}/f/topico/{topic_id}"),
        "published": row.created_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    }))
}

/// `Followers` count only: who follows a forum stays private.
pub(crate) async fn followers_json(
    db: &PgPool,
    host: &str,
    handle: &str,
    forum: &FedForum,
) -> Value {
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM forum_follower WHERE forum_id = $1 AND accepted_at IS NOT NULL",
    )
    .bind(forum.id)
    .fetch_one(db)
    .await
    .unwrap_or(0);
    json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "id": format!("https://{host}/actors/{handle}/followers"),
        "type": "OrderedCollection",
        "totalItems": total,
    })
}

/// An empty outbox: Lemmy fetches it when it first sees a community, to backfill.
pub(crate) fn empty_outbox_json(host: &str, handle: &str) -> Value {
    json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "id": format!("https://{host}/actors/{handle}/outbox"),
        "type": "OrderedCollection",
        "totalItems": 0,
        "orderedItems": [],
    })
}

/// Announce sweep (1/min in the worker): enqueues new topics × followers and
/// sends the pending ones signed with the forum's key. Idempotent per (topic, inbox).
pub async fn announce_sweep(db: &PgPool, public_origin: &str) {
    let origin = public_origin.trim_end_matches('/');
    let host = origin.trim_start_matches("https://");

    let _ = sqlx::query(
        r"INSERT INTO forum_announce_delivery (id, topic_id, recipient_inbox)
          SELECT gen_random_uuid(), t.id, ff.remote_inbox_url
          FROM forum_topic t
          JOIN forum f ON f.id = t.forum_id AND f.federated AND f.hidden_at IS NULL
          JOIN forum_follower ff ON ff.forum_id = t.forum_id AND ff.accepted_at IS NOT NULL
          WHERE t.hidden_at IS NULL AND t.remote_actor_url IS NULL
            AND t.created_at > now() - interval '7 days'
            AND forum_readable(f.id, NULL)
          ON CONFLICT (topic_id, recipient_inbox) DO NOTHING",
    )
    .execute(db)
    .await;

    let pending: Vec<(Uuid, Uuid, String, String)> = match sqlx::query_as(
        r"SELECT d.id, d.topic_id, d.recipient_inbox, f.full_path
          FROM forum_announce_delivery d
          JOIN forum_topic t ON t.id = d.topic_id
          JOIN forum f ON f.id = t.forum_id
          WHERE d.sent_at IS NULL AND d.attempts < 5
            AND f.federated AND forum_readable(f.id, NULL)
          ORDER BY d.created_at LIMIT 10",
    )
    .fetch_all(db)
    .await
    {
        Ok(r) => r,
        Err(err) => {
            tracing::warn!(?err, "forum announce: lookup falhou");
            return;
        }
    };

    for (delivery_id, topic_id, inbox, full_path) in pending {
        let handle = path_to_handle(&full_path);
        let actor = format!("https://{host}/actors/{handle}");
        let forum_id: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM forum WHERE full_path = $1")
            .bind(&full_path)
            .fetch_optional(db)
            .await
            .unwrap_or(None);
        let Some((forum_id,)) = forum_id else {
            continue;
        };
        let Some((_, private_pem)) = ensure_forum_keys(db, forum_id).await else {
            continue;
        };
        let Some(page) = topic_object_json(db, host, &handle, topic_id).await else {
            // Hidden since it was queued: there is nothing left to send.
            let _ = sqlx::query("UPDATE forum_announce_delivery SET attempts = 5 WHERE id = $1")
                .bind(delivery_id)
                .execute(db)
                .await;
            continue;
        };
        let announce = |id: String, object: Value| {
            json!({
                "@context": "https://www.w3.org/ns/activitystreams",
                "id": id,
                "type": "Announce",
                "actor": actor,
                "to": ["https://www.w3.org/ns/activitystreams#Public"],
                "cc": [format!("{actor}/followers")],
                "audience": actor,
                "object": object,
            })
        };
        // FEP-1b12 wraps the activity, which is what Lemmy and PieFed read. Mastodon and Pleroma
        // cannot read that and need the object itself; Lemmy sends both for the same reason.
        let wrapped = announce(
            format!("{actor}/activities/announce-create-{delivery_id}"),
            crate::forum_outbound::create_activity(&actor, &page, &topic_id.to_string()),
        );
        let plain = announce(
            format!("{actor}/activities/announce-{delivery_id}"),
            json!(format!("{actor}/objects/{topic_id}")),
        );
        let sent = match crate::federation::deliver_signed(&actor, &private_pem, &inbox, &wrapped)
            .await
        {
            Ok(()) => crate::federation::deliver_signed(&actor, &private_pem, &inbox, &plain).await,
            Err(err) => Err(err),
        };
        match sent {
            Ok(()) => {
                let _ = sqlx::query(
                    "UPDATE forum_announce_delivery SET sent_at = now() WHERE id = $1 AND sent_at IS NULL",
                )
                .bind(delivery_id)
                .execute(db)
                .await;
                tracing::info!(topic = %topic_id, inbox = %inbox, "forum: Announce entregue");
            }
            Err(err) => {
                let _ = sqlx::query(
                    "UPDATE forum_announce_delivery SET attempts = attempts + 1 WHERE id = $1",
                )
                .bind(delivery_id)
                .execute(db)
                .await;
                tracing::warn!(?err, inbox = %inbox, "forum: Announce falhou");
            }
        }
    }

    let config = crate::forum_inbound::InboundConfig::from_env();
    crate::forum_outbound::sweep(db, host, &config).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_path_round_trip() {
        assert_eq!(handle_to_path("sp").as_deref(), Some("sp"));
        assert_eq!(handle_to_path("santos.sp").as_deref(), Some("sp/santos"));
        assert_eq!(
            handle_to_path("saude.santos.sp").as_deref(),
            Some("sp/santos/saude")
        );
        assert_eq!(
            handle_to_path("ministerio-educacao").as_deref(),
            Some("ministerio-educacao")
        );
        assert_eq!(handle_to_path("ccj.senado").as_deref(), Some("senado/ccj"));
        assert_eq!(path_to_handle("sp/santos/saude"), "saude.santos.sp");
        assert_eq!(path_to_handle("senado/ccj"), "ccj.senado");
        // invalid
        assert!(handle_to_path("").is_none());
        assert!(handle_to_path("a..b").is_none());
        assert!(handle_to_path("Maiuscula.sp").is_none());
        assert!(handle_to_path("a.b.c.d").is_none());
    }
}

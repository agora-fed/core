//! # ActivityPub federation for consultas (type `Question`).
//!
//! A consulta (single-choice vote) maps to the AP `Question` type with `oneOf`
//! options. Remote instances can:
//!
//! 1. **Dereference** the object at `/actors/{handle}/objects/consulta-{uuid}`
//!    and render it as a poll in their native UI.
//! 2. **Vote** by sending `Create{Note{name: "<option>", inReplyTo: "<url>"}}`
//!    to the creator's inbox — the standard Mastodon poll-vote shape.
//!
//! Plebiscitos are NOT federated (SSO-only, concordo/neutro/discordo).

use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

/// URL prefix for consulta AP objects: `/actors/{handle}/objects/consulta-{uuid}`.
const CONSULTA_PREFIX: &str = "consulta-";

/// Try to parse `consulta-{uuid}` from the object id segment.
pub(crate) fn parse_consulta_id(id: &str) -> Option<Uuid> {
    id.strip_prefix(CONSULTA_PREFIX)
        .and_then(|s| Uuid::parse_str(s).ok())
}

/// Build the canonical AP object URL for a consulta.
pub(crate) fn consulta_object_url(host: &str, handle: &str, consultation_id: Uuid) -> String {
    format!("https://{host}/actors/{handle}/objects/consulta-{consultation_id}")
}

/// Serve the AP `Question` JSON for a consulta.
///
/// Returns `None` when the consultation doesn't exist, isn't a consulta kind,
/// or the handle doesn't match the owning forum.
pub(crate) async fn consulta_object_json(
    db: &PgPool,
    host: &str,
    handle: &str,
    consultation_id: Uuid,
) -> Option<Value> {
    let forum = crate::forum_federation::lookup_by_handle(db, handle).await?;

    #[derive(sqlx::FromRow)]
    struct Row {
        title: String,
        status: String,
        closes_at: DateTime<Utc>,
        created_at: DateTime<Utc>,
    }
    let row: Row = sqlx::query_as(
        r"SELECT c.title, c.status, c.closes_at, c.created_at
            FROM consultations_consultation c
           WHERE c.id = $1 AND c.kind = 'consulta' AND c.forum_id = $2",
    )
    .bind(consultation_id)
    .bind(forum.id)
    .fetch_optional(db)
    .await
    .ok()??;

    let questions: Vec<(Uuid, String, i32)> = sqlx::query_as(
        r"SELECT id, prompt, position FROM consultations_consultation_question
           WHERE consultation_id = $1 ORDER BY position",
    )
    .bind(consultation_id)
    .fetch_all(db)
    .await
    .ok()?;

    let qids: Vec<Uuid> = questions.iter().map(|(id, _, _)| *id).collect();

    let vote_counts: Vec<(Uuid, i64)> = sqlx::query_as(
        r"SELECT question_id, count(*) FROM consultation_vote
           WHERE question_id = ANY($1)
           GROUP BY question_id",
    )
    .bind(&qids)
    .fetch_all(db)
    .await
    .unwrap_or_default();
    let counts: std::collections::HashMap<Uuid, i64> = vote_counts.into_iter().collect();

    let total_voters: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM consultation_vote WHERE consultation_id = $1",
    )
    .bind(consultation_id)
    .fetch_one(db)
    .await
    .unwrap_or(0);

    let one_of: Vec<Value> = questions
        .iter()
        .map(|(qid, prompt, _)| {
            json!({
                "type": "Note",
                "name": prompt,
                "replies": {
                    "type": "Collection",
                    "totalItems": counts.get(qid).copied().unwrap_or(0),
                },
            })
        })
        .collect();

    let actor_url = format!("https://{host}/actors/{handle}");
    let object_url = consulta_object_url(host, handle, consultation_id);
    let origin = format!("https://{host}");

    let mut obj = json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "id": object_url,
        "type": "Question",
        "attributedTo": actor_url,
        "audience": actor_url,
        "content": format!("<p>{}</p>", html_escape(&row.title)),
        "published": row.created_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "endTime": row.closes_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "url": format!("{origin}/consulta/?id={consultation_id}"),
        "to": [&actor_url, "https://www.w3.org/ns/activitystreams#Public"],
        "cc": [format!("{actor_url}/followers")],
        "oneOf": one_of,
        "votersCount": total_voters,
    });

    if row.status == "closed" {
        obj["closed"] = Value::String(
            row.closes_at
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        );
    }

    Some(obj)
}

/// Handle an incoming federated vote on a consulta.
///
/// Mastodon sends votes as `Create{Note{name: "<option>", inReplyTo: "<question URL>"}}`.
/// Returns `true` if the activity was consumed as a consulta vote (so the caller skips
/// regular Note processing).
pub(crate) async fn handle_consulta_vote(
    db: &PgPool,
    signer_actor_url: &str,
    object: &Value,
) -> bool {
    let Some(reply_to) = object.get("inReplyTo").and_then(Value::as_str) else {
        return false;
    };
    let Some(option_name) = object.get("name").and_then(Value::as_str) else {
        return false;
    };

    let Some((consultation_id, question_id)) =
        resolve_vote_target(db, reply_to, option_name).await
    else {
        return false;
    };

    match sqlx::query(
        r"INSERT INTO consultation_vote
              (consultation_id, question_id, actor_url)
          VALUES ($1, $2, $3)
          ON CONFLICT ON CONSTRAINT consultation_vote_one_per_remote DO NOTHING",
    )
    .bind(consultation_id)
    .bind(question_id)
    .bind(signer_actor_url)
    .execute(db)
    .await
    {
        Ok(_) => {
            tracing::info!(
                actor = %signer_actor_url,
                %consultation_id,
                option = %option_name,
                "federated consulta vote recorded"
            );
            true
        }
        Err(err) => {
            tracing::error!(?err, actor = %signer_actor_url, "failed to record federated vote");
            true
        }
    }
}

/// Given a consulta object URL and an option name, resolve to (consultation_id, question_id).
async fn resolve_vote_target(
    db: &PgPool,
    reply_to_url: &str,
    option_name: &str,
) -> Option<(Uuid, Uuid)> {
    let consultation_id = parse_consulta_id_from_url(reply_to_url)?;

    let (kind, status): (String, String) = sqlx::query_as(
        "SELECT kind, status FROM consultations_consultation WHERE id = $1",
    )
    .bind(consultation_id)
    .fetch_optional(db)
    .await
    .ok()??;
    if kind != "consulta" || status != "open" {
        tracing::debug!(
            %consultation_id,
            %kind,
            %status,
            "federated vote rejected: not an open consulta"
        );
        return None;
    }

    let question: Option<Uuid> = sqlx::query_scalar(
        r"SELECT id FROM consultations_consultation_question
           WHERE consultation_id = $1 AND prompt = $2",
    )
    .bind(consultation_id)
    .bind(option_name)
    .fetch_optional(db)
    .await
    .ok()?;

    question.map(|qid| (consultation_id, qid))
}

/// Extract a consultation UUID from a consulta object URL.
/// Only matches our own origin to prevent cross-instance spoofing.
fn parse_consulta_id_from_url(url: &str) -> Option<Uuid> {
    let origin = std::env::var("PUBLIC_ORIGIN")
        .unwrap_or_else(|_| "https://democracia.social.br".to_owned());
    let base = origin.trim_end_matches('/');
    if !url.starts_with(base) {
        return None;
    }
    let segment = url.rsplit('/').next()?;
    parse_consulta_id(segment)
}

/// Wrap a `Question` object in a `Create` activity from the Group actor.
pub(crate) fn create_activity(actor_url: &str, question: &Value, consultation_id: Uuid) -> Value {
    json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "id": format!("{actor_url}/activities/create-consulta-{consultation_id}"),
        "type": "Create",
        "actor": actor_url,
        "to": question.get("to").cloned().unwrap_or(json!([])),
        "cc": question.get("cc").cloned().unwrap_or(json!([])),
        "audience": actor_url,
        "published": question.get("published").cloned().unwrap_or(json!(null)),
        "object": question,
    })
}

/// Outbox items for a forum: `Create{Question}` for each open or recently closed consulta.
pub(crate) async fn outbox_items(
    db: &PgPool,
    host: &str,
    handle: &str,
) -> (Vec<Value>, i64) {
    let forum = match crate::forum_federation::lookup_by_handle(db, handle).await {
        Some(f) => f,
        None => return (vec![], 0),
    };

    let rows: Vec<(Uuid,)> = sqlx::query_as(
        r"SELECT c.id FROM consultations_consultation c
          WHERE c.kind = 'consulta' AND c.forum_id = $1
          ORDER BY c.created_at DESC LIMIT 40",
    )
    .bind(forum.id)
    .fetch_all(db)
    .await
    .unwrap_or_default();

    let total = rows.len() as i64;
    let mut items = Vec::with_capacity(rows.len());
    for (cid,) in rows {
        if let Some(obj) = consulta_object_json(db, host, handle, cid).await {
            let actor_url = format!("https://{host}/actors/{handle}");
            items.push(create_activity(&actor_url, &obj, cid));
        }
    }
    (items, total)
}

/// Sweep: enqueue delivery rows for open consultas × accepted followers, then send pending.
pub(crate) async fn consulta_announce_sweep(db: &PgPool, host: &str) {
    let _ = sqlx::query(
        r"INSERT INTO consulta_announce_delivery (consultation_id, recipient_inbox)
          SELECT c.id, ff.remote_inbox_url
          FROM consultations_consultation c
          JOIN forum f ON f.id = c.forum_id AND f.federated
          JOIN forum_follower ff ON ff.forum_id = c.forum_id AND ff.accepted_at IS NOT NULL
          WHERE c.kind = 'consulta' AND c.status = 'open' AND c.closes_at > now()
            AND c.forum_id IS NOT NULL
          ON CONFLICT (consultation_id, recipient_inbox) DO NOTHING",
    )
    .execute(db)
    .await;

    let pending: Vec<(Uuid, Uuid, String, Option<Uuid>)> = match sqlx::query_as(
        r"SELECT d.id, d.consultation_id, d.recipient_inbox, c.forum_id
          FROM consulta_announce_delivery d
          JOIN consultations_consultation c ON c.id = d.consultation_id
          WHERE d.sent_at IS NULL AND d.attempts < 5
            AND c.kind = 'consulta' AND c.forum_id IS NOT NULL
          ORDER BY d.created_at LIMIT 10",
    )
    .fetch_all(db)
    .await
    {
        Ok(r) => r,
        Err(err) => {
            tracing::warn!(?err, "consulta announce: lookup failed");
            return;
        }
    };

    for (delivery_id, consultation_id, inbox, forum_id) in pending {
        let Some(forum_id) = forum_id else { continue };

        let forum = match crate::forum_federation::lookup_by_handle_by_id(db, forum_id).await {
            Some(f) => f,
            None => continue,
        };
        let handle = crate::forum_federation::path_to_handle(&forum.full_path);
        let actor_url = format!("https://{host}/actors/{handle}");

        let Some((_, private_pem)) =
            crate::forum_federation::ensure_forum_keys(db, forum_id).await
        else {
            continue;
        };

        let Some(question) = consulta_object_json(db, host, &handle, consultation_id).await else {
            let _ = sqlx::query(
                "UPDATE consulta_announce_delivery SET attempts = 5 WHERE id = $1",
            )
            .bind(delivery_id)
            .execute(db)
            .await;
            continue;
        };

        let activity = create_activity(&actor_url, &question, consultation_id);

        match crate::federation::deliver_signed(&actor_url, &private_pem, &inbox, &activity).await
        {
            Ok(()) => {
                let _ = sqlx::query(
                    "UPDATE consulta_announce_delivery SET sent_at = now() WHERE id = $1 AND sent_at IS NULL",
                )
                .bind(delivery_id)
                .execute(db)
                .await;
                tracing::info!(
                    %consultation_id, inbox = %inbox,
                    "consulta: Create(Question) delivered"
                );
            }
            Err(err) => {
                let _ = sqlx::query(
                    "UPDATE consulta_announce_delivery SET attempts = attempts + 1 WHERE id = $1",
                )
                .bind(delivery_id)
                .execute(db)
                .await;
                tracing::warn!(?err, inbox = %inbox, "consulta: Create(Question) delivery failed");
            }
        }
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_activity_shape() {
        let cid = Uuid::parse_str("56a3eb5f-a036-410e-8e6a-8b7bd32769ea").unwrap();
        let actor = "https://agora.organica.social/actors/propostas";
        let question = json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "id": format!("{actor}/objects/consulta-{cid}"),
            "type": "Question",
            "attributedTo": actor,
            "to": [actor, "https://www.w3.org/ns/activitystreams#Public"],
            "cc": [format!("{actor}/followers")],
            "published": "2026-11-03T11:59:19Z",
            "oneOf": [],
        });

        let activity = create_activity(actor, &question, cid);

        assert_eq!(activity["type"], "Create");
        assert_eq!(activity["actor"], actor);
        assert_eq!(activity["object"]["type"], "Question");
        assert_eq!(activity["object"]["attributedTo"], actor);
        assert_eq!(
            activity["id"],
            format!("{actor}/activities/create-consulta-{cid}")
        );
        assert!(activity["to"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "https://www.w3.org/ns/activitystreams#Public"));

        let activity2 = create_activity(actor, &question, cid);
        assert_eq!(activity["id"], activity2["id"], "id must be stable");
    }

    #[test]
    fn parse_consulta_id_round_trip() {
        let id = Uuid::parse_str("56a3eb5f-a036-410e-8e6a-8b7bd32769ea").unwrap();
        let url = consulta_object_url("agora.organica.social", "propostas", id);
        assert!(url.contains("consulta-56a3eb5f"));
        let segment = url.rsplit('/').next().unwrap();
        assert_eq!(parse_consulta_id(segment), Some(id));
    }

    #[test]
    fn html_escape_covers_basics() {
        assert_eq!(html_escape("<b>&\"x\"</b>"), "&lt;b&gt;&amp;&quot;x&quot;&lt;/b&gt;");
    }
}

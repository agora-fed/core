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
/// or the handle doesn't match the creator.
pub(crate) async fn consulta_object_json(
    db: &PgPool,
    host: &str,
    handle: &str,
    consultation_id: Uuid,
) -> Option<Value> {
    #[derive(sqlx::FromRow)]
    struct Row {
        title: String,
        status: String,
        closes_at: DateTime<Utc>,
        created_at: DateTime<Utc>,
        creator_handle: Option<String>,
    }
    let row: Row = sqlx::query_as(
        r"SELECT c.title, c.status, c.closes_at, c.created_at,
                 ci.handle AS creator_handle
            FROM consultations_consultation c
            LEFT JOIN citizen ci ON ci.id = c.created_by
           WHERE c.id = $1 AND c.kind = 'consulta'",
    )
    .bind(consultation_id)
    .fetch_optional(db)
    .await
    .ok()??;

    let creator_handle = row.creator_handle.as_deref()?;
    if creator_handle != handle {
        return None;
    }

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
        "content": format!("<p>{}</p>", html_escape(&row.title)),
        "published": row.created_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "endTime": row.closes_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "url": format!("{origin}/consulta/?id={consultation_id}"),
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
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

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

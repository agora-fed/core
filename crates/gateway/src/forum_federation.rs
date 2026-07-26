//! # F4 — fórum como ator `Group` do fediverso.
//!
//! Handle SEM prefixo (decisão 2026-07-26): segmentos do caminho INVERTIDOS
//! unidos por ponto — `@ministerio-educacao@`, `@sp@`, `@saude.sp@` (secretaria),
//! `@santos.sp@` (cidade), `@saude.santos.sp@` (seção municipal), `@ccj.senado@`.
//! Resolução SEMPRE tenta cidadão primeiro (compat); fórum é o fallback.
//!
//! Fluxos: WebFinger/actor Group (federation.rs delega pra cá) · Follow→Accept
//! assinado com a chave do fórum (geração preguiçosa) · Announce dos tópicos aos
//! seguidores (fan-out varrido pelo worker, 1x por inbox) · Note dereferenciável
//! do tópico em `/actors/<handle>/objects/<topic_id>`.

use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

/// `saude.santos.sp` → `sp/santos/saude`. `None` se não parecer handle de fórum.
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

/// Um fórum resolvido pra federação.
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

/// Resolve um handle federado num fórum visível (federated + não oculto).
pub(crate) async fn lookup_by_handle(db: &PgPool, handle: &str) -> Option<FedForum> {
    let path = handle_to_path(handle)?;
    sqlx::query_as(
        r"SELECT id, full_path, name, description, federated, avatar_url, banner_url
          FROM forum WHERE full_path = $1 AND hidden_at IS NULL",
    )
    .bind(&path)
    .fetch_optional(db)
    .await
    .ok()
    .flatten()
    .filter(|f: &FedForum| f.federated)
}

/// Chave do Group — geração preguiçosa, corrida-safe (UPDATE guardado + re-select).
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

/// JRD do WebFinger pra um handle de fórum.
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

/// Documento do ator `Group`.
pub(crate) fn group_actor_json(
    host: &str,
    handle: &str,
    forum: &FedForum,
    public_pem: &str,
) -> Value {
    let actor = format!("https://{host}/actors/{handle}");
    json!({
        "@context": ["https://www.w3.org/ns/activitystreams", "https://w3id.org/security/v1"],
        "id": actor,
        "type": "Group",
        "preferredUsername": handle,
        "name": forum.name,
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
        "publicKey": {
            "id": format!("{actor}#main-key"),
            "owner": actor,
            "publicKeyPem": public_pem,
        },
        "icon": forum.avatar_url.as_ref().map(|u| json!({
            "type": "Image", "url": absolutize_url(host, u)
        })),
        "image": forum.banner_url.as_ref().map(|u| json!({
            "type": "Image", "url": absolutize_url(host, u)
        })),
    })
}

/// `/media/x.png` → `https://<host>/media/x.png`; URLs absolutas passam intactas.
fn absolutize_url(host: &str, url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        url.to_owned()
    } else {
        format!("https://{}/{}", host, url.trim_start_matches('/'))
    }
}

/// Inbox do Group: `Follow` → registra seguidor (inbox REAL vindo do actor doc,
/// não do payload — limita spoof) e devolve `Accept` assinado pela chave do fórum;
/// `Undo{Follow}` → remove. Demais atividades: 202 ignoradas (v1 — comentários
/// remotos entram na fase seguinte, com moderação).
pub(crate) async fn inbox(
    db: &PgPool,
    host: &str,
    handle: &str,
    forum: &FedForum,
    activity: &Value,
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
            // Busca o ator remoto (fetch assinado pela instância) e usa o inbox DECLARADO.
            let Ok(doc) = crate::federation::fetch_remote_actor(actor_url).await else {
                tracing::warn!(actor_url, "forum follow: ator remoto não resolveu");
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
            // Accept assinado com a chave do Group — sem ele o Mastodon fica em "pendente".
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
                        tracing::warn!(?err, inbox = %inbox2, "forum follow: Accept falhou");
                    }
                });
            }
            tracing::info!(forum = %forum.full_path, remote = %actor_url, "forum: novo seguidor fediverso");
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
            }
            StatusCode::ACCEPTED
        }
        _ => StatusCode::ACCEPTED,
    }
}

/// Note AP dereferenciável de um tópico (`/actors/<handle>/objects/<topic_id>`).
pub(crate) async fn topic_object_json(
    db: &PgPool,
    host: &str,
    handle: &str,
    topic_id: Uuid,
) -> Option<Value> {
    let row: Option<(
        String,
        String,
        chrono::DateTime<chrono::Utc>,
        Option<String>,
    )> = sqlx::query_as(
        r"SELECT t.title, t.body, t.created_at, c.handle
              FROM forum_topic t
              JOIN forum f ON f.id = t.forum_id
              JOIN citizen c ON c.id = t.author_id
              WHERE t.id = $1 AND t.hidden_at IS NULL AND f.full_path = $2",
    )
    .bind(topic_id)
    .bind(handle_to_path(handle)?)
    .fetch_optional(db)
    .await
    .ok()?;
    let (title, body, created_at, author_handle) = row?;
    let actor = format!("https://{host}/actors/{handle}");
    let esc = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let content = format!(
        "<p><strong>{}</strong></p><p>{}</p>",
        esc(&title),
        esc(&body).replace('\n', "<br/>")
    );
    let attributed = author_handle
        .map(|h| format!("https://{host}/actors/{h}"))
        .unwrap_or_else(|| actor.clone());
    Some(json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "id": format!("{actor}/objects/{topic_id}"),
        "type": "Note",
        "attributedTo": attributed,
        "audience": actor,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "cc": [format!("{actor}/followers")],
        "name": title,
        "content": content,
        "url": format!("https://{host}/f/topico/{topic_id}"),
        "published": created_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    }))
}

/// Sweep do Announce (1/min no worker): enfileira tópicos novos × seguidores e
/// envia os pendentes assinando com a chave do fórum. Idempotente por (tópico, inbox).
pub(crate) async fn announce_sweep(db: &PgPool, public_origin: &str) {
    let origin = public_origin.trim_end_matches('/');
    let host = origin.trim_start_matches("https://");

    let _ = sqlx::query(
        r"INSERT INTO forum_announce_delivery (id, topic_id, recipient_inbox)
          SELECT gen_random_uuid(), t.id, ff.remote_inbox_url
          FROM forum_topic t
          JOIN forum f ON f.id = t.forum_id AND f.federated AND f.hidden_at IS NULL
          JOIN forum_follower ff ON ff.forum_id = t.forum_id AND ff.accepted_at IS NOT NULL
          WHERE t.hidden_at IS NULL AND t.created_at > now() - interval '7 days'
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
        let announce = json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "id": format!("{actor}/activities/announce-{delivery_id}"),
            "type": "Announce",
            "actor": actor,
            "to": ["https://www.w3.org/ns/activitystreams#Public"],
            "cc": [format!("{actor}/followers")],
            "object": format!("{actor}/objects/{topic_id}"),
        });
        match crate::federation::deliver_signed(&actor, &private_pem, &inbox, &announce).await {
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
        // inválidos
        assert!(handle_to_path("").is_none());
        assert!(handle_to_path("a..b").is_none());
        assert!(handle_to_path("Maiuscula.sp").is_none());
        assert!(handle_to_path("a.b.c.d").is_none());
    }
}

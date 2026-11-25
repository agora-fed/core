//! Tell a person that a room owner added them to a DIOGENES articulation room (ADR-0020).
//!
//! DIOGENES calls `Identity::member_added` after the membership is committed, only for a new
//! member. The host then:
//! 1. writes a `room_member` row in `user_notification` (the bell);
//! 2. sends a web push;
//! 3. sends the `diogenes_room_member` e-mail, unless `email_prefs.room_member = false`.
//!
//! Everything is best-effort: the member is already saved, so a failure here is only logged.
//! The recipient must belong to the org of the room; anything else is dropped (defence in
//! depth — DIOGENES only adds members found in the caller's org).

use std::collections::HashMap;

use sqlx::PgPool;
use uuid::Uuid;

use crate::notifications::{self, NewNotification};

/// Notification kind and e-mail opt-out key.
pub const KIND: &str = "room_member";
const TEMPLATE: &str = "diogenes_room_member";

/// The e-mail to send, already rendered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomMail {
    pub to: String,
    pub subject: String,
    pub body: String,
}

/// Public label of a room role.
#[must_use]
pub fn role_label(role: &str) -> &'static str {
    match role {
        "owner" => "responsável",
        "editor" => "editor(a)",
        _ => "leitor(a)",
    }
}

/// The page of the room.
#[must_use]
pub fn room_url(origin: &str, room_id: Uuid) -> String {
    format!(
        "{}/articulacao/#/sala/{room_id}",
        origin.trim_end_matches('/')
    )
}

/// Record the in-app notification and build the e-mail.
///
/// Returns `Ok(None)` when the recipient is not in the room's org, has no e-mail address,
/// or turned the e-mail off.
///
/// # Errors
/// A database error.
pub async fn record(
    db: &PgPool,
    origin: &str,
    n: &diogenes::MemberAdded,
) -> Result<Option<RoomMail>, sqlx::Error> {
    type Row = (Option<String>, serde_json::Value);
    let recipient: Option<Row> = sqlx::query_as(
        "SELECT ac.email, c.email_prefs
           FROM citizen c
           LEFT JOIN auth_credential ac ON ac.citizen_id = c.id
          WHERE c.id = $1 AND c.org_id = $2 AND c.deleted_at IS NULL",
    )
    .bind(n.user_id)
    .bind(n.org_id)
    .fetch_optional(db)
    .await?;
    let Some((email, prefs)) = recipient else {
        tracing::warn!(user = %n.user_id, org = %n.org_id, "diogenes notice: recipient not in org");
        return Ok(None);
    };
    let added_by_handle: Option<String> =
        sqlx::query_scalar("SELECT handle FROM citizen WHERE id = $1")
            .bind(n.added_by)
            .fetch_optional(db)
            .await?
            .flatten();
    let url = room_url(origin, n.room_id);
    let role = role_label(&n.role);
    let preview = format!(
        "{} adicionou você à sala \"{}\" como {role}",
        n.added_by_name, n.room_name
    );
    notifications::insert(
        db,
        NewNotification {
            citizen_id: n.user_id,
            kind: KIND,
            source_actor_url: None,
            source_handle: added_by_handle.as_deref().unwrap_or(&n.added_by_name),
            source_display_name: Some(&n.added_by_name),
            source_avatar_url: None,
            object_uri: Some(&url),
            object_preview: Some(&preview),
        },
    )
    .await?;
    let wants_mail = prefs
        .get(KIND)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let Some(to) = email.filter(|_| wants_mail) else {
        return Ok(None);
    };
    let mut vars: HashMap<&str, String> = HashMap::new();
    vars.insert("added_by", n.added_by_name.clone());
    vars.insert("room_name", n.room_name.clone());
    vars.insert("role_label", role.to_owned());
    vars.insert("room_url", url.clone());
    vars.insert(
        "prefs_url",
        format!(
            "{}/configuracoes#preferencias",
            origin.trim_end_matches('/')
        ),
    );
    let (subject, body) = match dsoc_db::email_templates::render(db, TEMPLATE, &vars).await {
        Some(rendered) => rendered,
        None => fallback_mail(&preview, &url),
    };
    Ok(Some(RoomMail { to, subject, body }))
}

/// The e-mail when the editable template is missing (it never blocks the notice).
fn fallback_mail(preview: &str, url: &str) -> (String, String) {
    (
        preview.to_owned(),
        format!("Olá,\n\n{preview}.\n\nAbra a sala:\n{url}"),
    )
}

/// Push payload for the web push of the notice.
#[must_use]
pub fn push_payload(origin: &str, n: &diogenes::MemberAdded) -> String {
    serde_json::json!({
        "title": "Você entrou em uma sala de articulação",
        "body": format!("{} adicionou você à sala \"{}\"", n.added_by_name, n.room_name),
        "url": room_url(origin, n.room_id),
    })
    .to_string()
}

/// Do all of it in the background.
pub fn spawn(db: PgPool, origin: String, n: diogenes::MemberAdded) {
    tokio::spawn(async move {
        crate::web_push::send_to_citizen(&db, n.user_id, &push_payload(&origin, &n)).await;
        match record(&db, &origin, &n).await {
            Ok(Some(mail)) => send(mail).await,
            Ok(None) => {}
            Err(err) => tracing::error!(error = ?err, "diogenes notice failed"),
        }
    });
}

async fn send(mail: RoomMail) {
    let Some(cfg) = crate::proposal_delivery::smtp_from_env() else {
        tracing::info!("SMTP unset; DIOGENES room e-mail not sent");
        return;
    };
    if let Err(err) = crate::mailer::send_html(&cfg, &mail.to, &mail.subject, &mail.body).await {
        tracing::error!(?err, "DIOGENES room e-mail failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice() -> diogenes::MemberAdded {
        let id = Uuid::nil();
        diogenes::MemberAdded {
            org_id: id,
            room_id: id,
            room_name: "PL 1".into(),
            user_id: id,
            role: "editor".into(),
            added_by: id,
            added_by_name: "Ana".into(),
        }
    }

    #[test]
    fn labels_and_urls() {
        assert_eq!(role_label("owner"), "responsável");
        assert_eq!(role_label("editor"), "editor(a)");
        assert_eq!(role_label("viewer"), "leitor(a)");
        assert_eq!(
            room_url("https://x.br/", Uuid::nil()),
            "https://x.br/articulacao/#/sala/00000000-0000-0000-0000-000000000000"
        );
    }

    #[tokio::test]
    async fn without_smtp_the_mail_is_skipped() {
        assert!(crate::proposal_delivery::smtp_from_env().is_none());
        send(RoomMail {
            to: "a@example.org".into(),
            subject: "s".into(),
            body: "b".into(),
        })
        .await;
    }

    #[test]
    fn fallback_mail_links_to_the_room() {
        let (subject, body) = fallback_mail("Ana adicionou você", "https://x.br/s");
        assert_eq!(subject, "Ana adicionou você");
        assert!(body.contains("https://x.br/s"));
    }

    #[test]
    fn push_names_the_room_and_links_to_it() {
        let v: serde_json::Value =
            serde_json::from_str(&push_payload("https://x.br", &notice())).unwrap();
        assert_eq!(v["body"], "Ana adicionou você à sala \"PL 1\"");
        assert!(v["url"]
            .as_str()
            .unwrap()
            .ends_with("/articulacao/#/sala/00000000-0000-0000-0000-000000000000"));
    }
}

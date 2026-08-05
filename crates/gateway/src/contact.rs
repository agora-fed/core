//! # Public contact form (0.28.1).
//!
//! `POST /contact` — the site's only contact channel. No e-mail address
//! is published in HTML (the steward's anti-harvesting decision): the
//! institutional pages link `/contato/?setor=…` and the message is
//! forwarded via SMTP (the same sovereign relay as the notifications) to the
//! caixa interna `CONTACT_INBOX`.
//!
//! Defences — a public endpoint, no authentication:
//! - honeypot: a filled `website` field → 200 "ok" without sending anything;
//! - in-memory rate limit per IP (`X-Forwarded-For`, behind Caddy),
//!   capped at `CONTACT_RATE_MAX_PER_HOUR` (default 5) — a single pod, no DB;
//! - setor fechado em enum + limites de tamanho por campo.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::extract::Json;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde::Deserialize;

use crate::proposal_delivery::{smtp_from_env, SmtpConfig};

const RATE_WINDOW: Duration = Duration::from_secs(3600);
const DEFAULT_RATE_MAX_PER_HOUR: usize = 5;

const MAX_NAME: usize = 120;
const MAX_EMAIL: usize = 254;
const MAX_SUBJECT: usize = 180;
const MAX_MESSAGE: usize = 5_000;

pub fn routes(_state: AppState) -> Router<()> {
    Router::new().route("/contact", post(submit))
}

#[derive(Debug, Deserialize)]
pub struct ContactBody {
    sector: String,
    name: String,
    email: String,
    subject: String,
    message: String,
    /// Honeypot — a human never sees the field; a bot that fills it receives a
    /// fake "ok" and the message is discarded.
    #[serde(default)]
    website: String,
}

/// Sectors exposed on the institutional pages. Closed here — the label
/// goes into the Subject for triage in the destination mailbox.
fn sector_label(sector: &str) -> Option<&'static str> {
    match sector {
        "contato" => Some("Contato geral"),
        "lgpd" => Some("LGPD/DPO"),
        "moderacao" => Some("Moderação"),
        "seguranca" => Some("Segurança"),
        "imprensa" => Some("Imprensa"),
        _ => None,
    }
}

fn caller_ip(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

static RATE: LazyLock<Mutex<HashMap<String, Vec<Instant>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// `true` when the IP still has quota in the window. A `None` IP (no
/// X-Forwarded-For) never blocks — the same posture as the signup rate limit.
fn rate_allow(ip: Option<&str>) -> bool {
    let Some(ip) = ip else { return true };
    let max = std::env::var("CONTACT_RATE_MAX_PER_HOUR")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_RATE_MAX_PER_HOUR);
    let now = Instant::now();
    let mut map = RATE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    map.retain(|_, hits| {
        hits.retain(|t| now.duration_since(*t) < RATE_WINDOW);
        !hits.is_empty()
    });
    let hits = map.entry(ip.to_owned()).or_default();
    if hits.len() >= max {
        return false;
    }
    hits.push(now);
    true
}

fn fail(status: StatusCode, code: &str, msg: &str) -> Response {
    (status, Json(ApiResponse::<()>::fail(code, msg))).into_response()
}

fn validate(body: &ContactBody) -> Result<&'static str, Box<Response>> {
    let sector = sector_label(&body.sector).ok_or_else(|| {
        Box::new(fail(
            StatusCode::BAD_REQUEST,
            "invalid_sector",
            "Setor inválido.",
        ))
    })?;
    let name = body.name.trim();
    let email = body.email.trim();
    let subject = body.subject.trim();
    let message = body.message.trim();
    let email_shape = email.len() >= 5
        && email.len() <= MAX_EMAIL
        && email.contains('@')
        && !email.contains(char::is_whitespace);
    if name.len() < 2 || name.len() > MAX_NAME {
        return Err(Box::new(fail(
            StatusCode::BAD_REQUEST,
            "invalid_name",
            "Informe seu nome (2 a 120 caracteres).",
        )));
    }
    if !email_shape {
        return Err(Box::new(fail(
            StatusCode::BAD_REQUEST,
            "invalid_email",
            "Informe um e-mail válido para resposta.",
        )));
    }
    if subject.len() < 3 || subject.len() > MAX_SUBJECT {
        return Err(Box::new(fail(
            StatusCode::BAD_REQUEST,
            "invalid_subject",
            "Informe um assunto (3 a 180 caracteres).",
        )));
    }
    if message.len() < 10 || message.len() > MAX_MESSAGE {
        return Err(Box::new(fail(
            StatusCode::BAD_REQUEST,
            "invalid_message",
            "A mensagem precisa ter entre 10 e 5.000 caracteres.",
        )));
    }
    Ok(sector)
}

async fn submit(headers: HeaderMap, Json(body): Json<ContactBody>) -> Response {
    // Honeypot: answer success with no effect — never teach the bot.
    if !body.website.trim().is_empty() {
        return (StatusCode::OK, Json(ApiResponse::ok(()))).into_response();
    }
    let sector = match validate(&body) {
        Ok(s) => s,
        Err(resp) => return *resp,
    };
    if !rate_allow(caller_ip(&headers).as_deref()) {
        return fail(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "Muitas mensagens deste endereço. Tente novamente em uma hora.",
        );
    }
    let Some(cfg) = smtp_from_env() else {
        tracing::error!("contact form: SMTP not configured, message dropped");
        return fail(
            StatusCode::SERVICE_UNAVAILABLE,
            "contact_unavailable",
            "O envio está temporariamente indisponível. Tente mais tarde.",
        );
    };
    let inbox = std::env::var("CONTACT_INBOX").unwrap_or_else(|_| "sysadmin@pop.coop".into());
    let subject = format!("[Contato/{sector}] {}", body.subject.trim());
    let text = format!(
        "Setor: {sector}\nNome: {}\nE-mail: {}\n\n{}\n",
        body.name.trim(),
        body.email.trim(),
        body.message.trim(),
    );
    match send_with_reply_to(&cfg, &inbox, body.email.trim(), &subject, &text).await {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::ok(()))).into_response(),
        Err(err) => {
            tracing::error!(%err, "contact form: SMTP send failed");
            fail(
                StatusCode::SERVICE_UNAVAILABLE,
                "contact_unavailable",
                "O envio está temporariamente indisponível. Tente mais tarde.",
            )
        }
    }
}

/// A variant of [`crate::proposal_delivery`]'s `send_email` with `Reply-To`
/// do remetente humano — responder na caixa de destino responde direto
/// a quem escreveu.
async fn send_with_reply_to(
    cfg: &SmtpConfig,
    to: &str,
    reply_to: &str,
    subject: &str,
    body: &str,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use lettre::message::header::ContentType;
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::transport::smtp::AsyncSmtpTransport;
    use lettre::{AsyncTransport, Message, Tokio1Executor};

    let mut builder = if cfg.port == 465 {
        AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.host)?
    } else {
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.host)?
    };
    builder = builder.port(cfg.port).timeout(Some(Duration::from_secs(5)));
    if let (Some(u), Some(p)) = (cfg.user.as_ref(), cfg.pass.as_ref()) {
        builder = builder.credentials(Credentials::new(u.clone(), p.clone()));
    }
    let mailer = builder.build();

    let email = Message::builder()
        .from(cfg.from.parse()?)
        .to(to.parse()?)
        .reply_to(reply_to.parse()?)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_owned())?;
    mailer.send(email).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(sector: &str, message: &str) -> ContactBody {
        ContactBody {
            sector: sector.into(),
            name: "Cidadã Exemplo".into(),
            email: "cidada@example.org".into(),
            subject: "Dúvida sobre a plataforma".into(),
            message: message.into(),
            website: String::new(),
        }
    }

    #[test]
    fn accepts_known_sectors_rejects_unknown() {
        for s in ["contato", "lgpd", "moderacao", "seguranca"] {
            assert!(validate(&body(s, "mensagem com tamanho válido")).is_ok());
        }
        assert!(validate(&body("marketing", "mensagem com tamanho válido")).is_err());
    }

    #[test]
    fn rejects_short_message_and_bad_email() {
        assert!(validate(&body("contato", "curta")).is_err());
        let mut b = body("contato", "mensagem com tamanho válido");
        b.email = "sem-arroba".into();
        assert!(validate(&b).is_err());
    }

    #[test]
    fn rate_limit_caps_per_ip_and_ignores_missing_ip() {
        for _ in 0..DEFAULT_RATE_MAX_PER_HOUR {
            assert!(rate_allow(Some("203.0.113.7")));
        }
        assert!(!rate_allow(Some("203.0.113.7")));
        assert!(rate_allow(Some("203.0.113.8")));
        assert!(rate_allow(None));
    }
}

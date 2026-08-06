//! # INTERCOMS — the external-communication vertical (ADR-0016).
//!
//! A single, **pluggable** outbound layer: features talk to the
//! [`MessageSender`] trait, not to a specific provider. Increment 1 (#68): the trait,
//! o [`Channel`], e o [`SmtpProvider`] (e-mail via SMTP/lettre — o `mailer::send_html`
//! now delegates here). Next steps (same issue #68 / #69): extract into a
//! shared crate (so auth can use it for OTP/2FA), `MailgunProvider` + bulk
//! bulk, `SmsGatewayProvider`, encrypted per-scope config and rate limiting (SMS 1/week).

use async_trait::async_trait;

use crate::proposal_delivery::SmtpConfig;

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Symmetric key to encrypt/decrypt per-scope provider config (pgcrypto, #69).
/// Comes from `INTERCOMS_CONFIG_KEY` (a Secret). `None` = the per-directory config feature is unavailable.
pub(crate) fn config_key() -> Option<String> {
    std::env::var("INTERCOMS_CONFIG_KEY")
        .ok()
        .filter(|k| !k.is_empty())
}

/// Outbound delivery channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Email,
    Sms,
}

/// An outbound message. `body` is **plain text**; the e-mail provider wraps it
/// in the brand's HTML (`email_templates::html_wrap`). `subject` is ignored by SMS.
#[derive(Debug, Clone)]
pub struct OutboundMessage {
    pub channel: Channel,
    /// An e-mail address or an E.164 phone number, depending on the channel.
    pub to: String,
    pub subject: String,
    pub body: String,
}

impl OutboundMessage {
    /// Build an e-mail message.
    pub fn email(
        to: impl Into<String>,
        subject: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            channel: Channel::Email,
            to: to.into(),
            subject: subject.into(),
            body: body.into(),
        }
    }
}

/// Pluggable send provider. Features consume `&dyn MessageSender`.
#[async_trait]
pub trait MessageSender: Send + Sync {
    async fn send(&self, msg: &OutboundMessage) -> Result<(), BoxError>;
}

/// E-mail provider via SMTP (lettre), through the sovereign relay — INTERCOMS'
/// 1st provider. Sends multipart (plain text + the brand's HTML). The `Email` channel only.
#[derive(Debug)]
pub struct SmtpProvider {
    cfg: SmtpConfig,
}

impl SmtpProvider {
    pub(crate) fn new(cfg: SmtpConfig) -> Self {
        Self { cfg }
    }

    /// Read the config from the environment (`SMTP_*`); `None` when the relay is not configured.
    /// (used by the next phases — F3 broadcast, F6 OTP.)
    #[allow(dead_code)]
    pub(crate) fn from_env() -> Option<Self> {
        crate::proposal_delivery::smtp_from_env().map(Self::new)
    }
}

#[async_trait]
impl MessageSender for SmtpProvider {
    async fn send(&self, msg: &OutboundMessage) -> Result<(), BoxError> {
        if msg.channel != Channel::Email {
            return Err("SmtpProvider só envia e-mail".into());
        }
        use std::time::Duration;

        use lettre::message::MultiPart;
        use lettre::transport::smtp::authentication::Credentials;
        use lettre::transport::smtp::AsyncSmtpTransport;
        use lettre::{AsyncTransport, Message, Tokio1Executor};

        let cfg = &self.cfg;
        let mut builder = if cfg.port == 465 {
            AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.host)?
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.host)?
        };
        builder = builder.port(cfg.port).timeout(Some(Duration::from_secs(8)));
        if let (Some(u), Some(p)) = (cfg.user.as_ref(), cfg.pass.as_ref()) {
            builder = builder.credentials(Credentials::new(u.clone(), p.clone()));
        }
        let email = Message::builder()
            .from(cfg.from.parse()?)
            .to(msg.to.parse()?)
            .subject(msg.subject.clone())
            .multipart(MultiPart::alternative_plain_html(
                msg.body.clone(),
                dsoc_db::email_templates::html_wrap(&msg.body),
            ))?;
        builder.build().send(email).await?;
        Ok(())
    }
}

/// Config of an SMSGateway (the sms-gate.app Android app) — message endpoint URL + basic auth.
/// Increment 1 (#69): config from env (platform level). Per-scope config (directory/campaign)
/// encrypted = the next step.
#[derive(Clone)]
pub(crate) struct SmsConfig {
    pub url: String,
    pub user: Option<String>,
    pub pass: Option<String>,
}

/// MANUAL `Debug` (not derived): `user`/`pass` are the SMSGateway's basic auth.
/// Only the credential's presence is observable, never its value.
impl std::fmt::Debug for SmsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmsConfig")
            .field("url", &self.url)
            .field("user", &self.user.as_ref().map(|_| "<redacted>"))
            .field("pass", &self.pass.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

pub(crate) fn sms_from_env() -> Option<SmsConfig> {
    let url = std::env::var("SMS_GATEWAY_URL").ok()?;
    Some(SmsConfig {
        url,
        user: std::env::var("SMS_GATEWAY_USER").ok(),
        pass: std::env::var("SMS_GATEWAY_PASS").ok(),
    })
}

/// SMS provider via SMSGateway (the sms-gate.app Android app). POSTs JSON
/// `{message, phoneNumbers:[...]}` with basic auth. The `Sms` channel only.
#[derive(Debug)]
pub struct SmsGatewayProvider {
    cfg: SmsConfig,
}

impl SmsGatewayProvider {
    pub(crate) fn new(cfg: SmsConfig) -> Self {
        Self { cfg }
    }

    /// Read the config from the environment (`SMS_GATEWAY_*`); `None` when no gateway is configured.
    pub(crate) fn from_env() -> Option<Self> {
        sms_from_env().map(Self::new)
    }
}

/// SMS provider destinations. Many self-hosted SMSGateway instances run on plain
/// HTTP inside an operator's own network, so `INTERCOMS_ALLOW_HTTP=true` exists —
/// but it is OFF by default and pairs with `INTERCOMS_ALLOWLIST`, because turning it
/// on is what re-opens the path this guard closes.
pub(crate) fn sms_policy() -> crate::outbound::OutboundPolicy {
    crate::outbound::OutboundPolicy {
        allow_http: std::env::var("INTERCOMS_ALLOW_HTTP").as_deref() == Ok("true"),
        ..Default::default()
    }
    .with_allowlist_from_env("INTERCOMS_ALLOWLIST")
}

#[async_trait]
impl MessageSender for SmsGatewayProvider {
    async fn send(&self, msg: &OutboundMessage) -> Result<(), BoxError> {
        if msg.channel != Channel::Sms {
            return Err("SmsGatewayProvider só envia SMS".into());
        }
        // Through the SSRF guard (issue #9). This path carries citizens' phone OTPs,
        // and the provider URL is operator-supplied config: a mistyped or hostile
        // entry must not become a POST of a live OTP into the pod's own network.
        let body = serde_json::to_vec(&serde_json::json!({
            "message": msg.body,
            "phoneNumbers": [msg.to],
        }))?;
        let mut headers = vec![("content-type".to_owned(), "application/json".to_owned())];
        if let (Some(u), Some(p)) = (self.cfg.user.as_ref(), self.cfg.pass.as_ref()) {
            use base64::Engine as _;
            let raw = base64::engine::general_purpose::STANDARD.encode(format!("{u}:{p}"));
            headers.push(("authorization".to_owned(), format!("Basic {raw}")));
        }
        let (status, _) =
            crate::outbound::guarded_post(&self.cfg.url, &headers, body, &sms_policy()).await?;
        if !(200..300).contains(&status) {
            return Err(format!("SMSGateway respondeu HTTP {status}").into());
        }
        Ok(())
    }
}

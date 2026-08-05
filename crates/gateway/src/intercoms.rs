//! # INTERCOMS — the external-communication vertical (ADR-0016).
//!
//! A single, **pluggable** outbound layer: features talk to the
//! [`MessageSender`] trait, not to a specific provider. Increment 1 (#68): the trait,
//! o [`Channel`], e o [`SmtpProvider`] (e-mail via SMTP/lettre — o `mailer::send_html`
//! now delegates here). Next steps (same issue #68 / #69): extract into a
//! shared crate (so auth can use it for OTP/2FA), `MailgunProvider` + bulk
//! massa, `SmsGatewayProvider`, config por escopo cifrada e rate-limit (SMS 1/semana).

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

/// Canal de envio outbound.
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

/// Config de um SMSGateway (app Android sms-gate.app) — URL do endpoint de mensagem + basic auth.
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

/// Provider de SMS via SMSGateway (app Android sms-gate.app). POST JSON
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

#[async_trait]
impl MessageSender for SmsGatewayProvider {
    async fn send(&self, msg: &OutboundMessage) -> Result<(), BoxError> {
        if msg.channel != Channel::Sms {
            return Err("SmsGatewayProvider só envia SMS".into());
        }
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()?;
        let mut req = client.post(&self.cfg.url).json(&serde_json::json!({
            "message": msg.body,
            "phoneNumbers": [msg.to],
        }));
        if let (Some(u), Some(p)) = (self.cfg.user.as_ref(), self.cfg.pass.as_ref()) {
            req = req.basic_auth(u, Some(p));
        }
        let resp = req.send().await?;
        if !resp.status().is_success() {
            return Err(format!("SMSGateway respondeu HTTP {}", resp.status()).into());
        }
        Ok(())
    }
}

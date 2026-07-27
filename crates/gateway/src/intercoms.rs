//! # INTERCOMS — vertical de comunicação externa (ADR-0016).
//!
//! Camada única e **plugável** de envio outbound: features falam com o trait
//! [`MessageSender`], não com um provider específico. Increment 1 (#68): o trait,
//! o [`Channel`], e o [`SmtpProvider`] (e-mail via SMTP/lettre — o `mailer::send_html`
//! passa a delegar aqui). Próximos passos (mesma issue #68 / #69): extrair para um
//! crate compartilhado (para o auth usar em OTP/2FA), `MailgunProvider` + disparo em
//! massa, `SmsGatewayProvider`, config por escopo cifrada e rate-limit (SMS 1/semana).

use async_trait::async_trait;

use crate::proposal_delivery::SmtpConfig;

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Canal de envio outbound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Email,
    Sms,
}

/// Uma mensagem outbound. `body` é **texto-plano**; o provider de e-mail embrulha
/// no HTML da marca (`email_templates::html_wrap`). `subject` é ignorado por SMS.
#[derive(Debug, Clone)]
pub struct OutboundMessage {
    pub channel: Channel,
    /// Endereço de e-mail ou telefone E.164, conforme o canal.
    pub to: String,
    pub subject: String,
    pub body: String,
}

impl OutboundMessage {
    /// Constrói uma mensagem de e-mail.
    pub fn email(to: impl Into<String>, subject: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            channel: Channel::Email,
            to: to.into(),
            subject: subject.into(),
            body: body.into(),
        }
    }
}

/// Provider plugável de envio. As features consomem `&dyn MessageSender`.
#[async_trait]
pub trait MessageSender: Send + Sync {
    async fn send(&self, msg: &OutboundMessage) -> Result<(), BoxError>;
}

/// Provider de e-mail via SMTP (lettre), pelo relay soberano — 1º provider do
/// INTERCOMS. Envia multipart (texto-plano + HTML da marca). Só o canal `Email`.
pub struct SmtpProvider {
    cfg: SmtpConfig,
}

impl SmtpProvider {
    pub(crate) fn new(cfg: SmtpConfig) -> Self {
        Self { cfg }
    }

    /// Lê a config do ambiente (`SMTP_*`); `None` quando o relay não está configurado.
    /// (usado pelas próximas fases — F3 broadcast, F6 OTP.)
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
/// Increment 1 (#69): config por env (nível plataforma). Config por escopo (diretório/campanha)
/// cifrada = próximo passo.
#[derive(Clone)]
pub(crate) struct SmsConfig {
    pub url: String,
    pub user: Option<String>,
    pub pass: Option<String>,
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
/// `{message, phoneNumbers:[...]}` com basic auth. Só o canal `Sms`.
pub struct SmsGatewayProvider {
    cfg: SmsConfig,
}

impl SmsGatewayProvider {
    pub(crate) fn new(cfg: SmsConfig) -> Self {
        Self { cfg }
    }

    /// Lê a config do ambiente (`SMS_GATEWAY_*`); `None` quando não há gateway configurado.
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

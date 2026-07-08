//! Recibo de entrega da proposta — subscriber do `ProposalCreated` que
//! dispara dois e-mails (autor + gabinete) e registra o timestamp de envio
//! em `proposal.notified_{author,mandate}_at`.
//!
//! Reutiliza o mesmo transport SMTP das outras notificações (password_reset,
//! signup_verify, mandate_invite). Sem SMTP configurado, loga o link em INFO
//! e ainda escreve o timestamp (dev-mode).
//!
//! # Contrato
//! - Ao receber `Event::ProposalCreated { proposal, mandate, .. }`:
//!   1. Resolve `author_citizen_id` + `title` + `body` da proposta.
//!   2. Resolve `email` do autor via `auth_credential`.
//!   3. Resolve `public_email` + `display_name` do mandato.
//!   4. Envia dois e-mails em paralelo (spawn — não bloqueia o dispatch loop).
//!   5. Grava os timestamps (idempotente: `IS NULL` guard evita re-write).
//!
//! # Nota LGPD
//! O e-mail pro autor tem título + trecho da própria proposta (dele mesmo).
//! O e-mail pro gabinete usa `mandate.public_email` (dado público oficial do
//! Câmara/Senado/TSE) — não é canal privado.

use async_trait::async_trait;
use dsoc_core::events::{Event, EventEnvelope};
use dsoc_core::Result;
use dsoc_events::EventHandler;
use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::AsyncSmtpTransport;
use lettre::{AsyncTransport, Message, Tokio1Executor};
use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug)]
pub struct ProposalDeliverySub {
    pub db: PgPool,
    pub public_origin: String,
}

#[async_trait]
impl EventHandler for ProposalDeliverySub {
    async fn handle(&self, envelope: &EventEnvelope) -> Result<()> {
        if let Event::ProposalCreated { proposal, mandate, .. } = envelope.event {
            self.dispatch(proposal.as_uuid(), mandate.as_uuid()).await;
        }
        Ok(())
    }
}

impl ProposalDeliverySub {
    async fn dispatch(&self, proposal_id: Uuid, _mandate_id: Uuid) {
        // Uma query só puxa tudo — a JOIN evita 3 idas ao banco.
        let row: Option<ProposalDeliveryRow> = match sqlx::query_as(
            r"SELECT p.title,
                     p.body,
                     p.author_citizen_id,
                     p.notified_author_at,
                     p.notified_mandate_at,
                     m.display_name         AS mandate_display_name,
                     m.public_email         AS mandate_email,
                     ac.email               AS author_email
                FROM proposal p
                LEFT JOIN mandate m ON m.id = p.mandate_id
                LEFT JOIN auth_credential ac ON ac.citizen_id = p.author_citizen_id
               WHERE p.id = $1",
        )
        .bind(proposal_id)
        .fetch_optional(&self.db)
        .await
        {
            Ok(r) => r,
            Err(err) => {
                tracing::warn!(?err, %proposal_id, "proposal_delivery: lookup falhou");
                return;
            }
        };
        let Some(row) = row else {
            return;
        };
        let proposal_url = format!(
            "{}/propostas/{}",
            self.public_origin.trim_end_matches('/'),
            proposal_id
        );
        let smtp = smtp_from_env();

        // AUTHOR — só se ainda não notificado E se tem e-mail.
        if row.notified_author_at.is_none() {
            if let Some(email) = row.author_email.as_deref() {
                self.send_and_stamp(
                    email,
                    &format!("Sua proposta foi registrada — {}", row.title),
                    &format!(
                        "Olá,\n\n\
                         Sua proposta \"{}\" foi registrada e enviada ao gabinete \
                         de {} para conhecimento.\n\n\
                         Acompanhe aqui:\n{}\n\n\
                         Quando outras pessoas apoiarem e o limiar for atingido, \
                         o relógio de resposta começa a correr. Silêncio vira \
                         registro público.\n\n\
                         — DemocraciaBR",
                        row.title,
                        row.mandate_display_name.as_deref().unwrap_or("(mandato)"),
                        proposal_url,
                    ),
                    &smtp,
                    proposal_id,
                    "notified_author_at",
                )
                .await;
            }
        }

        // MANDATE — só se ainda não notificado E se o mandato tem e-mail público.
        if row.notified_mandate_at.is_none() {
            if let Some(email) = row.mandate_email.as_deref() {
                let body_short: String = row.body.chars().take(400).collect();
                let ellipsis = if row.body.chars().count() > 400 { "…" } else { "" };
                self.send_and_stamp(
                    email,
                    &format!("[DemocraciaBR] Nova proposta cidadã — {}", row.title),
                    &format!(
                        "Olá,\n\n\
                         Você recebeu uma nova proposta cidadã pela DemocraciaBR, \
                         infraestrutura pública de accountability parlamentar.\n\n\
                         Título: {}\n\n\
                         Trecho:\n{}{}\n\n\
                         Leia o texto completo, veja o número de apoios e responda:\n\
                         {}\n\n\
                         Enviada por cidadã(o) verificada(o) da plataforma. \
                         Não é necessário responder por esta caixa — a resposta \
                         formal fica registrada dentro do link acima e conta pro \
                         placar público de accountability.\n\n\
                         — DemocraciaBR (sistema automático)",
                        row.title, body_short, ellipsis, proposal_url,
                    ),
                    &smtp,
                    proposal_id,
                    "notified_mandate_at",
                )
                .await;
            }
        }
    }

    async fn send_and_stamp(
        &self,
        to: &str,
        subject: &str,
        body: &str,
        smtp: &Option<SmtpConfig>,
        proposal_id: Uuid,
        column: &'static str,
    ) {
        let Some(cfg) = smtp else {
            tracing::info!(
                target: "proposal_delivery",
                to, subject, "DEV: SMTP unconfigured; e-mail logado em vez de enviado."
            );
            self.stamp_now(proposal_id, column).await;
            return;
        };
        let cfg = cfg.clone();
        let to_owned = to.to_owned();
        let subject_owned = subject.to_owned();
        let body_owned = body.to_owned();
        let db = self.db.clone();
        let column_owned: &'static str = column;
        tokio::spawn(async move {
            match send_email(&cfg, &to_owned, &subject_owned, &body_owned).await {
                Ok(()) => {
                    let _ = stamp_now(&db, proposal_id, column_owned).await;
                }
                Err(err) => {
                    tracing::warn!(?err, to = %to_owned, "proposal_delivery: SMTP falhou");
                }
            }
        });
    }

    async fn stamp_now(&self, proposal_id: Uuid, column: &'static str) {
        let _ = stamp_now(&self.db, proposal_id, column).await;
    }
}

async fn stamp_now(db: &PgPool, proposal_id: Uuid, column: &'static str) -> Result<()> {
    // column: literal validado no callsite; não vem do usuário.
    let sql = format!(
        "UPDATE proposal SET {} = now() WHERE id = $1 AND {} IS NULL",
        column, column
    );
    if let Err(err) = sqlx::query(&sql).bind(proposal_id).execute(db).await {
        tracing::warn!(?err, %proposal_id, column, "proposal_delivery: stamp falhou");
    }
    Ok(())
}

#[derive(sqlx::FromRow, Debug)]
struct ProposalDeliveryRow {
    title: String,
    body: String,
    #[allow(dead_code)]
    author_citizen_id: Option<Uuid>,
    notified_author_at: Option<chrono::DateTime<chrono::Utc>>,
    notified_mandate_at: Option<chrono::DateTime<chrono::Utc>>,
    mandate_display_name: Option<String>,
    mandate_email: Option<String>,
    author_email: Option<String>,
}

// ---------------------------------------------------------------------------
// SMTP — reaproveita config das outras crates (password_reset etc.), duplicado
// aqui pra evitar cross-crate coupling. Se um dia sair um `dsoc_mailer`
// centralizado, este bloco vai junto.
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct SmtpConfig {
    host: String,
    port: u16,
    user: Option<String>,
    pass: Option<String>,
    from: String,
}

fn smtp_from_env() -> Option<SmtpConfig> {
    let host = std::env::var("SMTP_HOST").ok()?;
    let from = std::env::var("SMTP_FROM").ok()?;
    let port = std::env::var("SMTP_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(587_u16);
    let user = std::env::var("SMTP_USER").ok();
    let pass = std::env::var("SMTP_PASS").ok();
    Some(SmtpConfig { host, port, user, pass, from })
}

async fn send_email(
    cfg: &SmtpConfig,
    to: &str,
    subject: &str,
    body: &str,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut builder = if cfg.port == 465 {
        AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.host)?
    } else {
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.host)?
    };
    builder = builder
        .port(cfg.port)
        .timeout(Some(Duration::from_secs(5)));
    if let (Some(u), Some(p)) = (cfg.user.as_ref(), cfg.pass.as_ref()) {
        builder = builder.credentials(Credentials::new(u.clone(), p.clone()));
    }
    let mailer = builder.build();

    let from = cfg.from.parse()?;
    let to_addr: lettre::message::Mailbox = to.parse()?;
    let email = Message::builder()
        .from(from)
        .to(to_addr)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_owned())?;
    mailer.send(email).await?;
    Ok(())
}

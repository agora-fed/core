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
use sqlx::PgPool;
use std::collections::HashMap;
use uuid::Uuid;

use crate::email_templates;

#[derive(Debug)]
pub struct ProposalDeliverySub {
    pub db: PgPool,
    pub public_origin: String,
}

#[async_trait]
impl EventHandler for ProposalDeliverySub {
    async fn handle(&self, envelope: &EventEnvelope) -> Result<()> {
        if let Event::ProposalCreated {
            proposal, mandate, ..
        } = envelope.event
        {
            self.dispatch(proposal.as_uuid(), mandate.as_uuid()).await;
        }
        Ok(())
    }
}

impl ProposalDeliverySub {
    async fn dispatch(&self, proposal_id: Uuid, _mandate_id: Uuid) {
        // Uma query só puxa proposta + autor — os destinatários vêm da proposal_target (0537).
        let row: Option<ProposalDeliveryRow> = match sqlx::query_as(
            r"SELECT p.title,
                     p.body,
                     p.mandate_id,
                     p.author_citizen_id,
                     p.notified_author_at,
                     p.notified_mandate_at,
                     ac.email               AS author_email
                FROM proposal p
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
        // Multi-destinatário (0537): todos os gabinetes da proposta, principal incluído
        // (o backfill da migration garante ao menos 1 linha pra propostas antigas).
        let targets: Vec<TargetDeliveryRow> = match sqlx::query_as(
            r"SELECT pt.mandate_id,
                     pt.notified_at,
                     m.display_name,
                     -- Integridade (A1/D4): placeholder da plataforma NÃO é canal real → NULL, pra
                     -- nunca entregar num inbox morto nem carimbar 'notificado' (silêncio seria NOSSO).
                     CASE WHEN m.public_email ILIKE '%@parlamento.democracia.social.br'
                          THEN NULL ELSE m.public_email END AS public_email
                FROM proposal_target pt
                JOIN mandate m ON m.id = pt.mandate_id
               WHERE pt.proposal_id = $1
               ORDER BY pt.created_at, pt.mandate_id",
        )
        .bind(proposal_id)
        .fetch_all(&self.db)
        .await
        {
            Ok(t) => t,
            Err(err) => {
                tracing::warn!(?err, %proposal_id, "proposal_delivery: targets lookup falhou");
                return;
            }
        };
        let proposal_url = format!(
            "{}/propostas/{}",
            self.public_origin.trim_end_matches('/'),
            proposal_id
        );
        let smtp = smtp_from_env();

        // Nome(s) pro e-mail do autor: "Fulana" ou "Fulana, Beltrano e Cicrana".
        let mandate_name = match targets.len() {
            0 => "(mandato)".to_owned(),
            1 => targets[0].display_name.clone(),
            n => {
                let names: Vec<&str> = targets.iter().map(|t| t.display_name.as_str()).collect();
                format!("{} e {}", names[..n - 1].join(", "), names[n - 1])
            }
        };
        let body_short: String = row.body.chars().take(400).collect::<String>()
            + if row.body.chars().count() > 400 {
                "…"
            } else {
                ""
            };

        // AUTHOR — só se ainda não notificado E se tem e-mail.
        if row.notified_author_at.is_none() {
            if let Some(email) = row.author_email.as_deref() {
                let mut ctx: HashMap<&str, String> = HashMap::new();
                ctx.insert("proposal_title", row.title.clone());
                ctx.insert("proposal_url", proposal_url.clone());
                ctx.insert("mandate_name", mandate_name.clone());
                let (subject, body) = email_templates::render(
                    &self.db,
                    "proposal_confirm_author",
                    &ctx,
                )
                .await
                .unwrap_or_else(|| {
                    // Fallback hardcoded — só se a linha na DB sumiu.
                    (
                        format!("Sua proposta foi registrada — {}", row.title),
                        format!(
                            "Sua proposta \"{}\" foi registrada e enviada ao gabinete de {}.\n\nAcompanhe: {}\n\n— DemocraciaBR",
                            row.title, mandate_name, proposal_url,
                        ),
                    )
                });
                self.send_and_stamp(
                    email,
                    &subject,
                    &body,
                    &smtp,
                    proposal_id,
                    "notified_author_at",
                )
                .await;
            }
        }

        // GABINETES (0537) — um e-mail por destinatário ainda não notificado que tenha
        // e-mail público. Cada envio carimba o recibo do PRÓPRIO gabinete; o principal
        // também carimba o legado `proposal.notified_mandate_at` (UI/clientes antigos).
        for target in &targets {
            if target.notified_at.is_some() {
                continue;
            }
            let Some(email) = target.public_email.as_deref() else {
                continue;
            };
            let mut ctx: HashMap<&str, String> = HashMap::new();
            ctx.insert("proposal_title", row.title.clone());
            ctx.insert("proposal_body_short", body_short.clone());
            ctx.insert("proposal_url", proposal_url.clone());
            let (subject, body) =
                email_templates::render(&self.db, "proposal_confirm_mandate", &ctx)
                    .await
                    .unwrap_or_else(|| {
                        (
                            format!("[DemocraciaBR] Nova proposta cidadã — {}", row.title),
                            format!(
                                "Nova proposta cidadã pela DemocraciaBR.\n\nTítulo: {}\n\nTrecho:\n{}\n\nLeia: {}\n\n— DemocraciaBR (sistema automático)",
                                row.title, body_short, proposal_url,
                            ),
                        )
                    });
            let stamp_legacy =
                target.mandate_id == row.mandate_id && row.notified_mandate_at.is_none();
            self.send_and_stamp_target(
                email,
                &subject,
                &body,
                &smtp,
                proposal_id,
                target.mandate_id,
                stamp_legacy,
            )
            .await;
        }
    }

    /// Retry sweep: reencontra propostas cujo e-mail (autor OU gabinete) não saiu
    /// — porque o SMTP falhou na 1ª tentativa e o cursor do evento já avançou
    /// (o envio é fire-and-forget e só carimba em sucesso) — e re-dispara a
    /// entrega. Idempotente: `dispatch` só reenvia o lado com `notified_*_at IS NULL`.
    /// Janela: propostas com > 90s (deixa a 1ª tentativa acontecer) e < 7 dias.
    pub async fn sweep_undelivered(&self) {
        let rows: Vec<(Uuid,)> = match sqlx::query_as(
            r"SELECT p.id
                FROM proposal p
               WHERE (p.notified_author_at IS NULL
                      OR EXISTS (SELECT 1 FROM proposal_target pt
                                  WHERE pt.proposal_id = p.id AND pt.notified_at IS NULL))
                 AND p.created_at < now() - interval '90 seconds'
                 AND p.created_at > now() - interval '7 days'
               ORDER BY p.created_at DESC
               LIMIT 50",
        )
        .fetch_all(&self.db)
        .await
        {
            Ok(r) => r,
            Err(err) => {
                tracing::warn!(?err, "proposal_delivery: sweep lookup falhou");
                return;
            }
        };
        if !rows.is_empty() {
            tracing::info!(count = rows.len(), "proposal_delivery: retry sweep");
        }
        for (proposal_id,) in rows {
            // `dispatch` re-resolve o mandato pela JOIN; o 2º arg é ignorado.
            self.dispatch(proposal_id, Uuid::nil()).await;
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

    /// Versão por-gabinete do [`Self::send_and_stamp`] (0537): em sucesso carimba
    /// `proposal_target.notified_at` do destinatário; `stamp_legacy` também carimba o
    /// `proposal.notified_mandate_at` (o destinatário principal, pra UI/clientes antigos).
    #[allow(clippy::too_many_arguments)]
    async fn send_and_stamp_target(
        &self,
        to: &str,
        subject: &str,
        body: &str,
        smtp: &Option<SmtpConfig>,
        proposal_id: Uuid,
        mandate_id: Uuid,
        stamp_legacy: bool,
    ) {
        let Some(cfg) = smtp else {
            tracing::info!(
                target: "proposal_delivery",
                to, subject, "DEV: SMTP unconfigured; e-mail logado em vez de enviado."
            );
            stamp_target_now(&self.db, proposal_id, mandate_id, stamp_legacy).await;
            return;
        };
        let cfg = cfg.clone();
        let to_owned = to.to_owned();
        let subject_owned = subject.to_owned();
        let body_owned = body.to_owned();
        let db = self.db.clone();
        tokio::spawn(async move {
            match send_email(&cfg, &to_owned, &subject_owned, &body_owned).await {
                Ok(()) => {
                    stamp_target_now(&db, proposal_id, mandate_id, stamp_legacy).await;
                }
                Err(err) => {
                    tracing::warn!(?err, to = %to_owned, "proposal_delivery: SMTP falhou");
                }
            }
        });
    }
}

/// Carimba o recibo por-gabinete (e, opcionalmente, o legado do principal). Idempotente:
/// ambos os UPDATEs têm guarda `IS NULL`.
async fn stamp_target_now(db: &PgPool, proposal_id: Uuid, mandate_id: Uuid, stamp_legacy: bool) {
    if let Err(err) = sqlx::query(
        "UPDATE proposal_target SET notified_at = now()
          WHERE proposal_id = $1 AND mandate_id = $2 AND notified_at IS NULL",
    )
    .bind(proposal_id)
    .bind(mandate_id)
    .execute(db)
    .await
    {
        tracing::warn!(?err, %proposal_id, %mandate_id, "proposal_delivery: stamp target falhou");
    }
    if stamp_legacy {
        let _ = stamp_now(db, proposal_id, "notified_mandate_at").await;
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
    /// Destinatário PRINCIPAL — usado só pra decidir o carimbo legado.
    mandate_id: Uuid,
    #[allow(dead_code)]
    author_citizen_id: Option<Uuid>,
    notified_author_at: Option<chrono::DateTime<chrono::Utc>>,
    notified_mandate_at: Option<chrono::DateTime<chrono::Utc>>,
    author_email: Option<String>,
}

/// Um destinatário pendente/entregue da proposta (JOIN proposal_target × mandate).
#[derive(sqlx::FromRow, Debug)]
struct TargetDeliveryRow {
    mandate_id: Uuid,
    notified_at: Option<chrono::DateTime<chrono::Utc>>,
    display_name: String,
    public_email: Option<String>,
}

// ---------------------------------------------------------------------------
// SMTP — reaproveita config das outras crates (password_reset etc.), duplicado
// aqui pra evitar cross-crate coupling. Se um dia sair um `dsoc_mailer`
// centralizado, este bloco vai junto.
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub(crate) struct SmtpConfig {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) user: Option<String>,
    pub(crate) pass: Option<String>,
    pub(crate) from: String,
}

pub(crate) fn smtp_from_env() -> Option<SmtpConfig> {
    let host = std::env::var("SMTP_HOST").ok()?;
    let from = std::env::var("SMTP_FROM").ok()?;
    let port = std::env::var("SMTP_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(587_u16);
    let user = std::env::var("SMTP_USER").ok();
    let pass = std::env::var("SMTP_PASS").ok();
    Some(SmtpConfig {
        host,
        port,
        user,
        pass,
        from,
    })
}

pub(crate) async fn send_email(
    cfg: &SmtpConfig,
    to: &str,
    subject: &str,
    body: &str,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Delega ao INTERCOMS (ADR-0016, #68): unifica o envio de e-mail do gateway no
    // `SmtpProvider`. Assinatura mantida — os ~4 callers (civic_notify, forum_mailer,
    // email_templates, federation) passam a mandar via INTERCOMS sem mudança.
    use crate::intercoms::{MessageSender, OutboundMessage, SmtpProvider};
    SmtpProvider::new(cfg.clone())
        .send(&OutboundMessage::email(to, subject, body))
        .await
}

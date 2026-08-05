//! Proposal delivery receipt — the `ProposalCreated` subscriber that
//! fires two e-mails (author + cabinet) and records the send timestamp
//! in `proposal.notified_{author,mandate}_at`.
//!
//! Reuses the same SMTP transport as the other notifications (password_reset,
//! signup_verify, mandate_invite). Without SMTP configured it logs the link at INFO
//! and still writes the timestamp (dev-mode).
//!
//! # Contrato
//! - On receiving `Event::ProposalCreated { proposal, mandate, .. }`:
//!   1. Resolve the proposal's `author_citizen_id` + `title` + `body`.
//!   2. Resolve `email` do autor via `auth_credential`.
//!   3. Resolve `public_email` + `display_name` do mandato.
//!   4. Sends two e-mails in parallel (spawn — never blocks the dispatch loop).
//!   5. Grava os timestamps (idempotente: `IS NULL` guard evita re-write).
//!
//! # Nota LGPD
//! The author's e-mail carries the title + an excerpt of their own proposal.
//! The office's e-mail uses `mandate.public_email` (official public data from the
//! legislature/electoral authority) — not a private channel.

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
        // A single query pulls proposal + author — recipients come from proposal_target (0537).
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
        // Multi-recipient (0537): every office of the proposal, primary included
        // (the migration's backfill guarantees at least 1 row for old proposals).
        let targets: Vec<TargetDeliveryRow> = match sqlx::query_as(
            r"SELECT pt.mandate_id,
                     pt.notified_at,
                     m.display_name,
                     -- Integrity (A1/D4): the platform placeholder is NOT a real channel → NULL, so we
                     -- never deliver to a dead inbox nor stamp 'notified' (the silence would be OURS).
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

        // Name(s) for the author's e-mail: "Fulana" or "Fulana, Beltrano and Cicrana".
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

        // AUTHOR — only when not yet notified AND an e-mail exists.
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
                    // Hardcoded fallback — only if the DB row vanished.
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

        // OFFICES (0537) — one e-mail per not-yet-notified recipient that has a
        // public e-mail. Each send stamps that office's OWN receipt; the primary
        // also stamps the legacy `proposal.notified_mandate_at` (old UI/clients).
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

    /// Retry sweep: finds proposals whose e-mail (author OR office) never went out
    /// — because SMTP failed on the 1st attempt and the event cursor already moved on
    /// (the send is fire-and-forget and only stamps on success) — and re-fires the
    /// delivery. Idempotent: `dispatch` only resends the side with `notified_*_at IS NULL`.
    /// Window: proposals older than 90s (letting the 1st attempt happen) and under 7 days.
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
            // `dispatch` re-resolves the mandate through the JOIN; the 2nd arg is ignored.
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

    /// Per-office version of [`Self::send_and_stamp`] (0537): on success it stamps
    /// the recipient's `proposal_target.notified_at`; `stamp_legacy` also stamps
    /// `proposal.notified_mandate_at` (the primary recipient, for old UI/clients).
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
/// both UPDATEs carry an `IS NULL` guard.
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
    // column: a literal validated at the call site; it never comes from the user.
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
    /// PRIMARY recipient — used only to decide the legacy stamp.
    mandate_id: Uuid,
    #[allow(dead_code)]
    author_citizen_id: Option<Uuid>,
    notified_author_at: Option<chrono::DateTime<chrono::Utc>>,
    notified_mandate_at: Option<chrono::DateTime<chrono::Utc>>,
    author_email: Option<String>,
}

/// A pending/delivered recipient of the proposal (JOIN proposal_target × mandate).
#[derive(sqlx::FromRow, Debug)]
struct TargetDeliveryRow {
    mandate_id: Uuid,
    notified_at: Option<chrono::DateTime<chrono::Utc>>,
    display_name: String,
    public_email: Option<String>,
}

// ---------------------------------------------------------------------------
// SMTP — reuses the other crates' config (password_reset etc.), duplicated
// here to avoid cross-crate coupling. If a centralized `dsoc_mailer` ever
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

/// MANUAL `Debug` (not derived): `user`/`pass` are the sovereign relay's credentials —
/// deriving it raw would leak the SMTP password in any `tracing::debug!`/panic. Only
/// their presence is observable.
impl std::fmt::Debug for SmtpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("from", &self.from)
            .field("user", &self.user.as_ref().map(|_| "<redacted>"))
            .field("pass", &self.pass.as_ref().map(|_| "<redacted>"))
            .finish()
    }
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
    // Delegates to INTERCOMS (ADR-0016, #68): unifies the gateway's e-mail sending in the
    // `SmtpProvider`. Assinatura mantida — os ~4 callers (civic_notify, forum_mailer,
    // email_templates, federation) now send through INTERCOMS with no change.
    use crate::intercoms::{MessageSender, OutboundMessage, SmtpProvider};
    SmtpProvider::new(cfg.clone())
        .send(&OutboundMessage::email(to, subject, body))
        .await
}

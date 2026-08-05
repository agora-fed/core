//! Sending plain-text e-mail through the sovereign SMTP relay — a single, reusable
//! helper (the same transport as `/contato` and password-reset). The config
//! comes from [`crate::proposal_delivery::smtp_from_env`]; when absent, the caller
//! decides (log in dev / refuse). Best-effort: errors return to the caller.

use crate::proposal_delivery::SmtpConfig;

/// Envia um e-mail multipart (texto-plano como fallback + HTML da marca via
/// [`dsoc_db::email_templates::html_wrap`]) — mesmo formato do mandate_invite /
/// password-reset. `body_text` is the already-rendered plain-text body (no HTML);
/// `html_wrap` wraps it in the layout with logo/footer. Errors come back as `Err`.
pub(crate) async fn send_html(
    cfg: &SmtpConfig,
    to: &str,
    subject: &str,
    body_text: &str,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Delega ao INTERCOMS (ADR-0016, #68): o envio real vive no `SmtpProvider`.
    // The signature is kept so existing callers are untouched.
    use crate::intercoms::{MessageSender, OutboundMessage, SmtpProvider};
    SmtpProvider::new(cfg.clone())
        .send(&OutboundMessage::email(to, subject, body_text))
        .await
}

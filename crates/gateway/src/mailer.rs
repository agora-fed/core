//! Envio de e-mail de texto simples pelo relay SMTP soberano — helper único e
//! reutilizável (mesmo transporte do `/contato` e do password-reset). A config
//! vem de [`crate::proposal_delivery::smtp_from_env`]; quando ausente, o chamador
//! decide (logar em dev / recusar). Best-effort: erros voltam pro chamador.

use crate::proposal_delivery::SmtpConfig;

/// Envia um e-mail multipart (texto-plano como fallback + HTML da marca via
/// [`dsoc_db::email_templates::html_wrap`]) — mesmo formato do mandate_invite /
/// password-reset. `body_text` é o corpo texto-plano já renderizado (sem HTML);
/// o `html_wrap` o embrulha no layout com logo/rodapé. Erros voltam como `Err`.
pub(crate) async fn send_html(
    cfg: &SmtpConfig,
    to: &str,
    subject: &str,
    body_text: &str,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Delega ao INTERCOMS (ADR-0016, #68): o envio real vive no `SmtpProvider`.
    // A assinatura é mantida para não tocar os callers existentes.
    use crate::intercoms::{MessageSender, OutboundMessage, SmtpProvider};
    SmtpProvider::new(cfg.clone())
        .send(&OutboundMessage::email(to, subject, body_text))
        .await
}

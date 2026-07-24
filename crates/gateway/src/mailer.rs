//! Envio de e-mail de texto simples pelo relay SMTP soberano — helper único e
//! reutilizável (mesmo transporte do `/contato` e do password-reset). A config
//! vem de [`crate::proposal_delivery::smtp_from_env`]; quando ausente, o chamador
//! decide (logar em dev / recusar). Best-effort: erros voltam pro chamador.

use std::time::Duration;

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
    use lettre::message::MultiPart;
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::transport::smtp::AsyncSmtpTransport;
    use lettre::{AsyncTransport, Message, Tokio1Executor};

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
        .to(to.parse()?)
        .subject(subject)
        .multipart(MultiPart::alternative_plain_html(
            body_text.to_owned(),
            dsoc_db::email_templates::html_wrap(body_text),
        ))?;
    builder.build().send(email).await?;
    Ok(())
}

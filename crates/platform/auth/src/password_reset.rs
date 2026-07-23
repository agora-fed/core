//! # Password reset — single-use, time-bound, e-mail-delivered (ADR-0010, W1).
//!
//! Request path: caller submits an e-mail → service generates a 256-bit URL-safe token, hashes
//! it (SHA-256), persists the hash with an expiry, and emits an e-mail carrying the plaintext.
//! Confirm path: caller submits `(token, new_password)` → service looks up the hash, verifies
//! it is live, swaps the credential's `password_hash`, and marks the token used. The plaintext
//! token never lives in any column.
//!
//! ## Safety choices
//! - **Hashed at rest.** Mirrors the `mandate_invitation` pattern (migration 0200) — a leaked DB
//!   never yields a usable reset link.
//! - **Single-use.** The `used_at` write is the only path to redeeming; a confirmed token cannot
//!   be re-played.
//! - **Time-bound** (1h by default, env-tunable). Stale links degrade safely.
//! - **Enumeration-resistant.** `request` returns success regardless of whether the e-mail is
//!   registered — the wire response carries no signal an attacker could use to map our user
//!   directory, while the legitimate user simply receives the e-mail (or not).
//! - **SMTP failure is logged, not exposed.** A misconfigured/down SMTP relay must not leak
//!   "this account exists" by becoming the only path that errors.

use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use dsoc_core::ids::OrgId;
use dsoc_core::{Error, Result};
use dsoc_db::Db;
use rand::RngCore;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::credential::hash_password;
use crate::queries;

/// Default reset-link lifetime (1 hour). Override with `AUTH_PASSWORD_RESET_TTL_SECS`.
const DEFAULT_RESET_TTL_SECS: i64 = 3_600;
/// Random-token byte length (256 bits → 43 chars base64url no-pad). Plenty against brute force.
const RESET_TOKEN_BYTES: usize = 32;
/// SMTP send timeout (so a hung relay does not stall the request — the user sees success either
/// way because of the enumeration-resistance guarantee).
const SMTP_TIMEOUT_SECS: u64 = 5;

/// Service for the password-reset flow. Holds the database, the clock, and an optional e-mail
/// transport. Build with [`PasswordResetService::from_env`] so the operator brings SMTP credentials
/// via the standard `SMTP_*` env vars without those values being baked into the binary.
#[derive(Clone)]
pub struct PasswordResetService {
    db: Db,
    clock: std::sync::Arc<dyn dsoc_core::Clock>,
    /// Public origin used when rendering the reset URL into the e-mail (`<origin>/redefinir-senha?token=…`).
    public_origin: String,
    /// SMTP configuration. `None` = dev mode (the URL is logged via tracing instead of sent).
    smtp: Option<SmtpConfig>,
    /// Reset-link lifetime, seconds.
    ttl_secs: i64,
}

#[derive(Clone)]
struct SmtpConfig {
    host: String,
    port: u16,
    user: Option<String>,
    pass: Option<String>,
    from: String,
}

impl std::fmt::Debug for PasswordResetService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PasswordResetService")
            .field("public_origin", &self.public_origin)
            .field("smtp_configured", &self.smtp.is_some())
            .field("ttl_secs", &self.ttl_secs)
            .finish_non_exhaustive()
    }
}

impl PasswordResetService {
    /// Build from `AppState` plus environment-sourced config. Recognized env vars:
    /// - `PUBLIC_ORIGIN` (default `https://democracia.social.br`)
    /// - `SMTP_HOST`, `SMTP_PORT` (default `587`), `SMTP_USER`, `SMTP_PASS`, `SMTP_FROM`
    /// - `AUTH_PASSWORD_RESET_TTL_SECS` (default 3600)
    #[must_use]
    pub fn from_state(state: &dsoc_app::AppState) -> Self {
        let public_origin = std::env::var("PUBLIC_ORIGIN")
            .unwrap_or_else(|_| "https://democracia.social.br".to_owned())
            .trim_end_matches('/')
            .to_owned();
        let ttl_secs = std::env::var("AUTH_PASSWORD_RESET_TTL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&v: &i64| v > 0)
            .unwrap_or(DEFAULT_RESET_TTL_SECS);
        let smtp = smtp_from_env();
        if smtp.is_none() {
            tracing::warn!(
                "SMTP_HOST/SMTP_FROM unset; password-reset e-mails will be LOGGED (dev) instead of sent."
            );
        }
        Self {
            db: state.db.clone(),
            clock: state.clock.clone(),
            public_origin,
            smtp,
            ttl_secs,
        }
    }

    /// Step 1 — request a reset for an e-mail address. **Always succeeds** at the wire level
    /// (enumeration-resistant): if the e-mail is unknown, we still report success and silently
    /// do nothing. A real user simply receives the e-mail; an attacker scraping the surface
    /// learns nothing about who is registered.
    ///
    /// `org` is needed because the same e-mail can exist in multiple tenants; we honor the
    /// caller's declared org.
    ///
    /// # Errors
    /// [`Error::Storage`] on a hard persistence failure.
    pub async fn request(&self, org: OrgId, email: &str, request_ip: Option<&str>) -> Result<()> {
        let normalized = email.trim().to_lowercase();
        // Best-effort e-mail validity check; an obviously bad string short-circuits without
        // touching the DB — but we still return Ok so the wire response carries no signal.
        if !normalized.contains('@') {
            return Ok(());
        }
        let cred = queries::find_credential_by_email(&self.db, org.as_uuid(), &normalized)
            .await
            .map_err(map_sqlx)?;
        let Some(cred) = cred else {
            // Unknown e-mail — no-op, no signal.
            return Ok(());
        };

        let now = self.clock.now();
        let token = generate_token();
        let hash = sha256(&token);
        let expires_at = now + chrono::Duration::seconds(self.ttl_secs);

        // Invalidate any live token for this citizen — at most one reset link is valid at a time.
        queries::password_reset_invalidate_live(&self.db, cred.citizen_id, now)
            .await
            .map_err(map_sqlx)?;

        queries::password_reset_insert(
            &self.db,
            Uuid::now_v7(),
            cred.citizen_id,
            &hash,
            expires_at,
            request_ip,
            now,
        )
        .await
        .map_err(map_sqlx)?;

        let url = format!("{}/redefinir-senha?token={}", self.public_origin, token);
        self.deliver_email(&normalized, &url).await;
        Ok(())
    }

    /// Step 2 — redeem a token and set the new password. The token is single-use: a confirmed
    /// row's `used_at` is set in the same transaction as the credential update, so a replay
    /// finds the row already used and is rejected.
    ///
    /// # Errors
    /// [`Error::Validation`] for an obviously bad password (min length 8);
    /// [`Error::Unauthorized`] for an invalid/expired/already-used token (deliberately opaque —
    /// the caller never learns which case applied);
    /// [`Error::Storage`] on persistence failure.
    pub async fn confirm(&self, token: &str, new_password: &str) -> Result<()> {
        if new_password.len() < 8 {
            return Err(Error::Validation(
                "senha deve ter ao menos 8 caracteres".to_owned(),
            ));
        }
        let hash = sha256(token);
        let now = self.clock.now();
        let row = queries::password_reset_find_live(&self.db, &hash, now)
            .await
            .map_err(map_sqlx)?
            .ok_or(Error::Unauthorized)?;

        let new_hash = hash_password(new_password)?;

        let mut tx = self.db.begin().await.map_err(map_sqlx)?;
        // Mark the reset row used FIRST inside the tx. If the credential update fails, the row
        // stays unused and the user can try again.
        queries::password_reset_mark_used(&mut *tx, row.id, now)
            .await
            .map_err(map_sqlx)?;
        queries::credential_update_password(&mut *tx, row.citizen_id, &new_hash)
            .await
            .map_err(map_sqlx)?;
        // Also kill every active session for this citizen — a leaked password must not leave a
        // foothold for the attacker via an existing cookie.
        queries::delete_all_sessions_for_citizen(&mut *tx, row.citizen_id)
            .await
            .map_err(map_sqlx)?;
        tx.commit().await.map_err(map_sqlx)?;
        Ok(())
    }

    async fn deliver_email(&self, to: &str, reset_url: &str) {
        let Some(smtp) = &self.smtp else {
            tracing::info!(
                target: "auth::password_reset",
                to,
                reset_url,
                "DEV: SMTP unconfigured; password-reset URL logged instead of sent."
            );
            return;
        };
        // Template editável pelo admin (0.32.0); fallback = texto original.
        let mut ctx: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
        ctx.insert("reset_url", reset_url.to_owned());
        let (subject, body) = dsoc_db::email_templates::render(&self.db, "password_reset", &ctx)
            .await
            .unwrap_or_else(|| {
                (
                    "DemocraciaBR — redefinição de senha".to_owned(),
                    format!(
                        "Olá,\n\nVocê (ou alguém) pediu para redefinir a senha da sua conta na DemocraciaBR.\n\
                         Para criar uma nova senha, abra este link em até 1 hora:\n\n{reset_url}\n\n\
                         Se não foi você, ignore esta mensagem — sua senha continua a mesma.\n\n\
                         — DemocraciaBR"
                    ),
                )
            });
        let to_owned = to.to_owned();
        let smtp = smtp.clone();
        // Send in a non-blocking task so the request returns even if the relay is slow. The
        // user already learned nothing about whether their e-mail was registered (the wire path
        // is the same either way), so a failed send is invisible to the wire and only audited.
        tokio::spawn(async move {
            if let Err(err) = send_email(&smtp, &to_owned, &subject, &body).await {
                tracing::error!(error = ?err, "password-reset e-mail send failed");
            }
        });
    }
}

/// Build SMTP config from env. Returns `None` if either `SMTP_HOST` or `SMTP_FROM` is missing.
fn smtp_from_env() -> Option<SmtpConfig> {
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

/// Lettre-backed send. STARTTLS on port 587 (the standard secure submission path); over port 465
/// uses implicit TLS. Authentication is enabled iff both user and pass are set.
async fn send_email(
    cfg: &SmtpConfig,
    to: &str,
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
    builder = builder
        .port(cfg.port)
        .timeout(Some(Duration::from_secs(SMTP_TIMEOUT_SECS)));
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

/// Generate a 256-bit URL-safe random token (43 chars base64url-no-pad).
fn generate_token() -> String {
    let mut buf = [0u8; RESET_TOKEN_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

/// SHA-256 of the token, returned as raw bytes (matches the `bytea` column).
fn sha256(token: &str) -> Vec<u8> {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    h.finalize().to_vec()
}

fn map_sqlx(error: sqlx::Error) -> Error {
    match error {
        sqlx::Error::RowNotFound => Error::NotFound("entity not found".to_owned()),
        other => Error::Storage(Box::new(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_are_distinct_and_url_safe() {
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b);
        assert_eq!(a.len(), 43); // 256 bits in base64url no-pad
        assert!(a
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn sha256_hashes_deterministically() {
        assert_eq!(sha256("abc"), sha256("abc"));
        assert_ne!(sha256("abc"), sha256("abd"));
        assert_eq!(sha256("").len(), 32);
    }
}

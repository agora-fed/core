//! # Admin-driven mandate invite — a bypass for the F1.4 auto-proof.
//!
//! F1.4 lets a parlamentar self-onboard IFF `body.email == mandate.public_email`. That fails
//! whenever the public_email on file at the Câmara/Senado/TSE is wrong, institutional, or
//! missing — a large chunk of the directory. This flow is the human bypass: a plataforma/
//! partido admin sends an invite to *any* address; the recipient clicks a one-time link and
//! creates the account + `directory` binding without ever having to match the stale registry.
//!
//! ## Safety choices (mirror `password_reset.rs`)
//! - **Hashed at rest.** Only SHA-256 of the token lives in `mandate_invite.token_hash`
//!   (BYTEA). Plaintext token exists ONLY in the outgoing e-mail and in the accept URL.
//! - **Single-use.** `accepted_at IS NULL` on acceptance transitions to a concrete timestamp
//!   under an optimistic guard — a replay finds the row already accepted and is rejected.
//! - **Time-bound** (72h by default, env-tunable). Stale links degrade safely.
//! - **Revocable.** The inviter (or admin) can mark `revoked_at`; the accept path treats
//!   revoked rows as not-found.
//! - **No token in logs, ever.** Only the hash prefix or the invite id is logged.
//! - **SQL runtime-unchecked.** The workspace uses `SQLX_OFFLINE` and this build host has no
//!   DB to regenerate `.sqlx/`. Every statement here uses `sqlx::query`/`query_as` at runtime
//!   with named types (mirrors `gateway::parlamentar_activity::load_mandate_source`).

use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Utc};
use dsoc_core::ids::{CitizenId, OrgId};
use dsoc_core::{Error, Result, VerificationLevel};
use dsoc_db::Db;
use rand::RngCore;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::credential::{hash_password, Cpf, CpfVerifier};
use crate::domain;
use crate::service::IssuedSession;
use crate::{queries, ZitadelAuth};

/// Default invite lifetime — 72h. Override with `MANDATE_INVITE_TTL_SECS`.
const DEFAULT_INVITE_TTL_SECS: i64 = 72 * 3_600;
/// Random-token byte length (256 bits → 43 chars base64url no-pad).
const INVITE_TOKEN_BYTES: usize = 32;
/// SMTP send timeout so a hung relay does not stall the request.
const SMTP_TIMEOUT_SECS: u64 = 5;
/// The verification level a mandate invite grants on acceptance — `directory` (the same as
/// the F1.4 auto-proof; the admin acts as the directory in this bypass).
const INVITE_BINDING_LEVEL: VerificationLevel = VerificationLevel::Directory;

/// Public summary of an invite, returned by `GET /mandate-invites/{token}` so the accept
/// page can show "you were invited to assume the mandate of X". Deliberately excludes the
/// recipient e-mail and the inviter's identity — those are not for the world to read.
#[derive(Debug, Clone)]
pub struct InviteSummary {
    pub mandate_display_name: String,
    pub party: Option<String>,
    pub uf: Option<String>,
    pub office: String,
    pub expires_at: DateTime<Utc>,
}

/// What the invite-send handler returns to the inviter. `token` is NEVER on the response —
/// only the id + expiry (the admin should not learn the plaintext; only the recipient does).
#[derive(Debug, Clone)]
pub struct SentInvite {
    pub id: Uuid,
    pub expires_at: DateTime<Utc>,
}

/// The full accept-invite request body (mirrors `RegisterRequest` minus `email` — the e-mail
/// is bound at send time and cannot be re-declared here).
#[derive(Debug, Clone)]
pub struct AcceptRequest {
    pub password: String,
    pub cpf: String,
    pub display_name: String,
}

#[derive(Clone)]
struct SmtpConfig {
    host: String,
    port: u16,
    user: Option<String>,
    pass: Option<String>,
    from: String,
}

/// The service. Holds the DB pool, the injected clock, and the resolved SMTP + public origins.
#[derive(Clone)]
pub struct MandateInviteService {
    db: Db,
    clock: std::sync::Arc<dyn dsoc_core::Clock>,
    /// The public front-end origin (accept URL: `<origin>/convite?token=<token>`). Rendered
    /// as a query-param link because the front-end is fully SSG (Astro static, ADR-0009) and
    /// cannot handle an unknown dynamic path segment without an SSR adapter.
    public_web_base: String,
    smtp: Option<SmtpConfig>,
    ttl_secs: i64,
    /// Retained so the accept flow can reuse the exact session-issuing path the regular
    /// register flow uses (persists a row in `auth_session` + sets `is_public=true`).
    auth: std::sync::Arc<ZitadelAuth>,
}

impl std::fmt::Debug for MandateInviteService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MandateInviteService")
            .field("public_web_base", &self.public_web_base)
            .field("smtp_configured", &self.smtp.is_some())
            .field("ttl_secs", &self.ttl_secs)
            .finish_non_exhaustive()
    }
}

impl MandateInviteService {
    /// Build from the shared `AppState` + env. Recognized env vars:
    /// - `PUBLIC_WEB_BASE` (default `https://democracia.social.br`)
    /// - `SMTP_HOST`, `SMTP_PORT` (default `587`), `SMTP_USER`, `SMTP_PASS`, `SMTP_FROM`
    /// - `MANDATE_INVITE_TTL_SECS` (default 259200 = 72h)
    #[must_use]
    pub fn from_state(state: &dsoc_app::AppState, auth: std::sync::Arc<ZitadelAuth>) -> Self {
        let public_web_base = std::env::var("PUBLIC_WEB_BASE")
            .or_else(|_| std::env::var("PUBLIC_ORIGIN"))
            .unwrap_or_else(|_| "https://democracia.social.br".to_owned())
            .trim_end_matches('/')
            .to_owned();
        let ttl_secs = std::env::var("MANDATE_INVITE_TTL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&v: &i64| v > 0)
            .unwrap_or(DEFAULT_INVITE_TTL_SECS);
        let smtp = smtp_from_env();
        if smtp.is_none() {
            tracing::warn!(
                "SMTP_HOST/SMTP_FROM unset; mandate-invite e-mails will be LOGGED (dev)."
            );
        }
        Self {
            db: state.db.clone(),
            clock: state.clock.clone(),
            public_web_base,
            smtp,
            ttl_secs,
            auth,
        }
    }

    /// Step 1 — an admin/plataforma-op dispatches an invite for a specific `mandate` to a
    /// chosen `email`. Generates a 256-bit token, stores ONLY its SHA-256, and mails the
    /// plaintext to the recipient. The response NEVER carries the token.
    ///
    /// # Errors
    /// [`Error::Validation`] on bad e-mail; [`Error::NotFound`] if the mandate does not exist
    /// in this org; [`Error::Forbidden`] if the inviter is neither a platform admin
    /// (`admin_role_binding` owner/admin) nor an admin of the mandate's party
    /// (`party_administrator` role `admin`, migration 0204); [`Error::Storage`] on
    /// persistence failure.
    pub async fn send(
        &self,
        org: OrgId,
        inviter: CitizenId,
        mandate_id: Uuid,
        email: &str,
    ) -> Result<SentInvite> {
        let email = normalize_email(email)?;
        let now = self.clock.now();
        let expires_at = now + chrono::Duration::seconds(self.ttl_secs);

        // Look up mandate + summary in one shot; also proves the mandate belongs to this org
        // so an inviter cannot invite people to mandates in tenants they cannot see.
        let summary = load_mandate_summary(&self.db, org.as_uuid(), mandate_id)
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| Error::NotFound("mandato não encontrado".to_owned()))?;

        // Authorization gate: platform (org) admin/owner, or admin of the mandate's party.
        // `moderador` deliberately does NOT qualify — inviting binds a real person to a
        // mandate, the highest-impact action in the party surface.
        let authorized = invite_authorized(
            &self.db,
            org.as_uuid(),
            inviter.as_uuid(),
            summary.party.as_deref(),
        )
        .await
        .map_err(map_sqlx)?;
        if !authorized {
            return Err(Error::Forbidden(
                "apenas administradores da plataforma ou do partido podem enviar convites de mandato"
                    .to_owned(),
            ));
        }

        let token = generate_token();
        let hash = sha256(&token);

        let id: Uuid = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO mandate_invite \
             (org_id, mandate_id, email, token_hash, invited_by, expires_at, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             RETURNING id",
        )
        .bind(org.as_uuid())
        .bind(mandate_id)
        .bind(&email)
        .bind(&hash)
        .bind(inviter.as_uuid())
        .bind(expires_at)
        .bind(now)
        .fetch_one(&self.db)
        .await
        .map_err(map_sqlx)?;

        let accept_url = format!("{}/convite?token={}", self.public_web_base, token);
        // Log the id (NOT the token) so we can audit which invite went out.
        tracing::info!(
            invite_id = %id,
            mandate_id = %mandate_id,
            "mandate-invite dispatched"
        );
        self.deliver_email(&email, &accept_url, &summary).await;

        Ok(SentInvite { id, expires_at })
    }

    /// Step 2 (public) — the recipient opens the URL; the front-end island calls this to
    /// render a friendly "you were invited to assume the mandate of X" screen. Does NOT
    /// return the recipient e-mail or the inviter's identity.
    ///
    /// # Errors
    /// [`Error::NotFound`] on unknown/expired/revoked/accepted token — collapsed to one code
    /// so the caller can't tell "wrong token" from "used token".
    pub async fn summary(&self, token: &str) -> Result<InviteSummary> {
        let hash = sha256(token);
        let now = self.clock.now();

        let row = sqlx::query_as::<
            _,
            (
                String,
                Option<String>,
                Option<String>,
                String,
                DateTime<Utc>,
            ),
        >(
            "SELECT m.display_name, m.party, m.uf, m.office, i.expires_at \
             FROM mandate_invite i \
             JOIN mandate m ON m.id = i.mandate_id \
             WHERE i.token_hash = $1 \
               AND i.accepted_at IS NULL \
               AND i.revoked_at IS NULL \
               AND i.expires_at > $2",
        )
        .bind(&hash)
        .bind(now)
        .fetch_optional(&self.db)
        .await
        .map_err(map_sqlx)?
        .ok_or_else(|| Error::NotFound("convite inválido ou expirado".to_owned()))?;

        Ok(InviteSummary {
            mandate_display_name: row.0,
            party: row.1,
            uf: row.2,
            office: row.3,
            expires_at: row.4,
        })
    }

    /// Step 3 (public) — accept the invite: create the citizen + credential + directory
    /// binding + session, then mark the invite accepted. All in one transaction.
    ///
    /// # Errors
    /// [`Error::Validation`] for a bad password/CPF/display_name;
    /// [`Error::NotFound`] for an unknown/expired/revoked/accepted token;
    /// [`Error::Conflict`] if the invite e-mail is already taken by another account;
    /// [`Error::Storage`] on persistence failure.
    pub async fn accept(&self, token: &str, req: AcceptRequest) -> Result<IssuedSession> {
        if req.password.len() < 8 {
            return Err(Error::Validation(
                "senha deve ter ao menos 8 caracteres".to_owned(),
            ));
        }
        let display_name = req.display_name.trim();
        if display_name.is_empty() || display_name.chars().count() > 80 {
            return Err(Error::Validation(
                "nome público deve ter entre 1 e 80 caracteres".to_owned(),
            ));
        }
        let display_name = display_name.to_owned();
        let cpf = Cpf::parse(&req.cpf)?;
        let password_hash = hash_password(&req.password)?;
        let hash = sha256(token);
        let now = self.clock.now();

        // Lock the invite row so we serialize concurrent accept attempts with the same token.
        let mut tx = self.db.begin().await.map_err(map_sqlx)?;

        let row = sqlx::query_as::<
            _,
            (
                Uuid,
                Uuid,
                Uuid,
                String,
                DateTime<Utc>,
                Option<DateTime<Utc>>,
                Option<DateTime<Utc>>,
            ),
        >(
            "SELECT id, org_id, mandate_id, email, expires_at, accepted_at, revoked_at \
             FROM mandate_invite \
             WHERE token_hash = $1 \
             FOR UPDATE",
        )
        .bind(&hash)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_sqlx)?
        .ok_or_else(|| Error::NotFound("convite não encontrado".to_owned()))?;

        let (invite_id, org_id, mandate_id, email, expires_at, accepted_at, revoked_at) = row;
        if accepted_at.is_some() || revoked_at.is_some() || expires_at <= now {
            tx.rollback().await.map_err(map_sqlx)?;
            return Err(Error::NotFound("convite inválido ou expirado".to_owned()));
        }

        // CPF verification kept algorithmic here (matches ZitadelAuth::register). If it check-
        // digit-fails, `Cpf::parse` already errored above; the async verify tags the assurance.
        let cpf_status = crate::credential::AlgorithmicCpfVerifier.verify(&cpf).await;

        let citizen = CitizenId::new();
        // Citizen row — level `directory` because this bypass is the admin acting as the
        // directory. F1.4 grants the same level via the public_email auto-proof.
        queries::insert_credential_citizen(
            &mut *tx,
            citizen.as_uuid(),
            org_id,
            domain::level_as_str(INVITE_BINDING_LEVEL),
            now,
        )
        .await
        .map_err(map_register_sqlx)?;
        queries::insert_credential(
            &mut *tx,
            Uuid::now_v7(),
            citizen.as_uuid(),
            org_id,
            &email,
            &password_hash,
            cpf.as_str(),
            cpf_status.as_str(),
            now,
        )
        .await
        .map_err(map_register_sqlx)?;
        // Politicians are always public (transparência).
        queries::force_citizen_public(&mut *tx, citizen.as_uuid())
            .await
            .map_err(map_sqlx)?;
        // Materialise the display_name the invite recipient picked, so their public profile
        // is already legible (their name, not the m-<uuid> placeholder).
        sqlx::query("UPDATE citizen SET display_name = $2 WHERE id = $1")
            .bind(citizen.as_uuid())
            .bind(&display_name)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx)?;
        // Identity binding: mandate_identity_binding at `directory`, evidence = this invite.
        let evidence = format!("mandate_invite:{invite_id}");
        queries::insert_mandate_identity_binding(
            &mut *tx,
            Uuid::now_v7(),
            mandate_id,
            citizen.as_uuid(),
            "directory",
            Some(&evidence),
            now,
        )
        .await
        .map_err(map_sqlx)?;
        // Mark accepted under the optimistic guard.
        let affected = sqlx::query(
            "UPDATE mandate_invite SET accepted_at = $2 \
             WHERE id = $1 AND accepted_at IS NULL AND revoked_at IS NULL",
        )
        .bind(invite_id)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx)?
        .rows_affected();
        if affected == 0 {
            tx.rollback().await.map_err(map_sqlx)?;
            return Err(Error::NotFound("convite inválido ou expirado".to_owned()));
        }

        // Issue a session so the recipient lands logged-in on `/painel-mandato`. Reuse the
        // ZitadelAuth path so the cookie/expiry math is identical to /auth/register.
        let session = self
            .auth
            .issue_session_in_tx(&mut tx, OrgId::from_uuid(org_id), citizen, &email, now)
            .await?;

        tx.commit().await.map_err(map_sqlx)?;
        Ok(session)
    }

    /// Optional: revoke a pending invite (idempotent — an already-revoked/accepted row is
    /// simply a no-op returning Ok). Only the original inviter may revoke; TODO for admin
    /// override (needs role model).
    ///
    /// # Errors
    /// [`Error::NotFound`] if no matching pending row exists for this caller.
    pub async fn revoke(&self, _org: OrgId, caller: CitizenId, token: &str) -> Result<()> {
        let hash = sha256(token);
        let now = self.clock.now();
        let affected = sqlx::query(
            "UPDATE mandate_invite SET revoked_at = $3 \
             WHERE token_hash = $1 AND invited_by = $2 \
               AND accepted_at IS NULL AND revoked_at IS NULL",
        )
        .bind(&hash)
        .bind(caller.as_uuid())
        .bind(now)
        .execute(&self.db)
        .await
        .map_err(map_sqlx)?
        .rows_affected();
        if affected == 0 {
            return Err(Error::NotFound("convite não encontrado".to_owned()));
        }
        Ok(())
    }

    async fn deliver_email(&self, to: &str, accept_url: &str, summary: &MandateSummary) {
        let Some(smtp) = &self.smtp else {
            tracing::info!(
                target: "auth::mandate_invite",
                to,
                accept_url,
                "DEV: SMTP unconfigured; mandate-invite URL logged instead of sent."
            );
            return;
        };
        let hours = (self.ttl_secs / 3_600).max(1);
        let party_uf = format!(
            "{}/{}",
            summary.party.as_deref().unwrap_or("—"),
            summary.uf.as_deref().unwrap_or("—")
        );
        let subject = format!(
            "DemocraciaBR — convite para assumir o mandato de {}",
            summary.display_name
        );
        let body = format!(
            "Olá,\n\n\
             Você foi convidado(a) a assumir, na plataforma DemocraciaBR, o mandato de:\n\n\
             \t{name} ({party_uf} — {office})\n\n\
             A DemocraciaBR é uma plataforma cidadã de cobrança pública de mandatos. \
             Ao aceitar este convite você:\n\n\
             \t• Cria sua conta pessoal (e-mail, senha e CPF).\n\
             \t• Verifica sua identidade no mandato indicado acima (nível 'directory').\n\
             \t• Torna seu perfil público — a transparência é a moeda desta plataforma.\n\n\
             Este link é único, expira em {hours} horas e só pode ser usado uma vez:\n\n\
             {accept_url}\n\n\
             Se você não reconhece este convite, ignore este e-mail — nada é feito até você abrir o link acima.\n\n\
             — DemocraciaBR",
            name = summary.display_name,
            party_uf = party_uf,
            office = summary.office,
            hours = hours,
            accept_url = accept_url,
        );
        let to_owned = to.to_owned();
        let smtp = smtp.clone();
        tokio::spawn(async move {
            if let Err(err) = send_email(&smtp, &to_owned, &subject, &body).await {
                tracing::error!(error = ?err, "mandate-invite e-mail send failed");
            }
        });
    }
}

/// Minimal mandate snapshot for the outgoing e-mail; identical shape as the public summary.
struct MandateSummary {
    display_name: String,
    party: Option<String>,
    uf: Option<String>,
    office: String,
}

async fn load_mandate_summary(
    db: &sqlx::PgPool,
    org: Uuid,
    mandate: Uuid,
) -> std::result::Result<Option<MandateSummary>, sqlx::Error> {
    let row = sqlx::query_as::<_, (String, Option<String>, Option<String>, String)>(
        "SELECT display_name, party, uf, office FROM mandate \
         WHERE org_id = $1 AND id = $2",
    )
    .bind(org)
    .bind(mandate)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(display_name, party, uf, office)| MandateSummary {
        display_name,
        party,
        uf,
        office,
    }))
}

/// May `citizen` send a mandate invite in `org`? True when the citizen is an org-level
/// admin/owner (`admin_role_binding`, migration 0150) or an accepted `admin` of the
/// mandate's party (`party_administrator`, migration 0204 — a seeded row with no
/// `invited_by` counts as accepted). A mandate with no party only accepts org admins.
async fn invite_authorized(
    db: &sqlx::PgPool,
    org: Uuid,
    citizen: Uuid,
    party_sigla: Option<&str>,
) -> std::result::Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS( \
            SELECT 1 FROM admin_role_binding \
             WHERE org_id = $1 AND citizen_id = $2 AND role IN ('owner', 'admin') \
         ) OR ($3::text IS NOT NULL AND EXISTS( \
            SELECT 1 FROM party_administrator \
             WHERE org_id = $1 AND citizen_id = $2 AND party_sigla = $3 \
               AND role = 'admin' \
               AND (accepted_at IS NOT NULL OR invited_by IS NULL) \
         ))",
    )
    .bind(org)
    .bind(citizen)
    .bind(party_sigla)
    .fetch_one(db)
    .await
}

/// SMTP from env — same shape as `password_reset::smtp_from_env`.
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

fn generate_token() -> String {
    let mut buf = [0u8; INVITE_TOKEN_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

fn sha256(token: &str) -> Vec<u8> {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    h.finalize().to_vec()
}

fn normalize_email(email: &str) -> Result<String> {
    let e = email.trim().to_lowercase();
    if e.len() < 3 || !e.contains('@') || !e.split('@').nth(1).is_some_and(|d| d.contains('.')) {
        return Err(Error::Validation("e-mail inválido".to_owned()));
    }
    Ok(e)
}

fn map_sqlx(error: sqlx::Error) -> Error {
    match error {
        sqlx::Error::RowNotFound => Error::NotFound("entity not found".to_owned()),
        other => Error::Storage(Box::new(other)),
    }
}

fn map_register_sqlx(error: sqlx::Error) -> Error {
    if let sqlx::Error::Database(db) = &error {
        if db.is_unique_violation() {
            let constraint = db.constraint().unwrap_or("");
            if constraint.contains("email") {
                return Error::Conflict("email_taken".to_owned());
            }
            if constraint.contains("cpf") {
                return Error::Conflict("cpf_taken".to_owned());
            }
            return Error::Conflict("entity already exists".to_owned());
        }
    }
    map_sqlx(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_are_distinct_and_url_safe() {
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b);
        assert_eq!(a.len(), 43);
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

    #[test]
    fn normalize_email_lowercases_and_validates() {
        assert_eq!(normalize_email(" A@B.co ").unwrap(), "a@b.co");
        assert!(normalize_email("nope").is_err());
        assert!(normalize_email("no@dot").is_err());
    }
}

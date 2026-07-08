//! # Signup verify — verificação de e-mail antes de criar a conta (0.25.0-fediverso).
//!
//! Fluxo antigo: `POST /auth/register` gravava citizen + credential + session
//! imediatamente. Isso significava que qualquer bot com CPF válido criava
//! conta viva sem provar controle do e-mail, e um CPF ficava travado por
//! e-mails errados/inválidos.
//!
//! Fluxo novo (`ADR-0011`):
//! 1. **Request** (`POST /auth/register` ou `.../politician`): normalizamos,
//!    validamos e hasheamos tudo, e persistimos numa `auth_pending_signup`
//!    (migration 0106) com um token SHA-256. **Nenhuma linha em `citizen`
//!    ainda.** Um e-mail com link `<origin>/confirmar-conta?token=…` sai
//!    pela infra SMTP existente (mesmo transport do password-reset).
//! 2. **Confirm** (`POST /auth/register/confirm`): recebemos o plaintext do
//!    link, achamos a pending pelo hash, e materializamos citizen + credential
//!    + sessão numa única transação (mark used, insert citizen, insert
//!    credential, [politico: mandate_binding + is_public=true], issue session).
//!
//! Espelha o pattern do [`crate::password_reset`] — mesmo hashing, mesma TTL
//! curta, mesmo cuidado de dev-mode logar o link quando SMTP não configurado.

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

use crate::credential::{hash_password, AlgorithmicCpfVerifier, Cpf, CpfVerifier};
use crate::domain;
use crate::queries;
use crate::service::IssuedSession;

/// Default TTL do link de verificação (24h). Override com `AUTH_SIGNUP_VERIFY_TTL_SECS`.
const DEFAULT_TTL_SECS: i64 = 24 * 3600;
/// Bytes aleatórios no token (256 bits → 43 chars base64url no-pad). Mesmo tamanho do password_reset.
const TOKEN_BYTES: usize = 32;
/// SMTP send timeout — não travar a requisição se o relay estiver lento.
const SMTP_TIMEOUT_SECS: u64 = 5;

/// Papel do cadastro pendente. Determina qual materialização o `confirm`
/// executa (register vs register_politician).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingRole {
    Cidadao,
    Politico,
}

impl PendingRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cidadao => "cidadao",
            Self::Politico => "politico",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "cidadao" => Some(Self::Cidadao),
            "politico" => Some(Self::Politico),
            _ => None,
        }
    }
}

#[derive(Clone)]
struct SmtpConfig {
    host: String,
    port: u16,
    user: Option<String>,
    pass: Option<String>,
    from: String,
}

/// Serviço da verificação de e-mail no cadastro. Segue o mesmo shape do
/// [`crate::password_reset::PasswordResetService`] — construção via
/// [`Self::from_state`] pra ler SMTP + origem pública do ambiente.
#[derive(Clone)]
pub struct SignupVerifyService {
    db: Db,
    clock: std::sync::Arc<dyn dsoc_core::Clock>,
    public_origin: String,
    smtp: Option<SmtpConfig>,
    ttl_secs: i64,
    session_ttl_secs: i64,
    cpf_verifier: std::sync::Arc<dyn CpfVerifier>,
}

impl std::fmt::Debug for SignupVerifyService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignupVerifyService")
            .field("public_origin", &self.public_origin)
            .field("smtp_configured", &self.smtp.is_some())
            .field("ttl_secs", &self.ttl_secs)
            .finish_non_exhaustive()
    }
}

impl SignupVerifyService {
    /// Constrói a partir do `AppState` + env. Env recognizadas:
    /// - `PUBLIC_ORIGIN` (default `https://democracia.social.br`)
    /// - `SMTP_HOST` / `SMTP_PORT` / `SMTP_USER` / `SMTP_PASS` / `SMTP_FROM`
    /// - `AUTH_SIGNUP_VERIFY_TTL_SECS` (default 86400)
    /// - `AUTH_SESSION_TTL_SECS` (default `DEFAULT_SESSION_TTL_SECS`, usado quando o confirm emite sessão)
    #[must_use]
    pub fn from_state(state: &dsoc_app::AppState) -> Self {
        let public_origin = std::env::var("PUBLIC_ORIGIN")
            .unwrap_or_else(|_| "https://democracia.social.br".to_owned())
            .trim_end_matches('/')
            .to_owned();
        let ttl_secs = std::env::var("AUTH_SIGNUP_VERIFY_TTL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&v: &i64| v > 0)
            .unwrap_or(DEFAULT_TTL_SECS);
        let session_ttl_secs = std::env::var("AUTH_SESSION_TTL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&v: &i64| v > 0)
            .unwrap_or(crate::domain::DEFAULT_SESSION_TTL_SECS);
        let smtp = smtp_from_env();
        if smtp.is_none() {
            tracing::warn!(
                "SMTP_HOST/SMTP_FROM unset; signup-verify e-mails will be LOGGED (dev) instead of sent."
            );
        }
        Self {
            db: state.db.clone(),
            clock: state.clock.clone(),
            public_origin,
            smtp,
            ttl_secs,
            session_ttl_secs,
            cpf_verifier: std::sync::Arc::new(AlgorithmicCpfVerifier),
        }
    }

    /// Passo 1a — cidadão comum. Normaliza + valida CPF + hasheia a senha +
    /// gera token + persiste pending + dispara e-mail. Sempre retorna sucesso
    /// no wire (mesma resistência a enumeração do password_reset): quem informou
    /// um e-mail existente descobre isso via *outro* caminho — o unique-violation
    /// só ocorre no confirm.
    ///
    /// # Errors
    /// [`Error::Validation`] pra e-mail/senha/CPF inválidos;
    /// [`Error::Storage`] em falha dura de persistência.
    pub async fn request_cidadao(
        &self,
        org: OrgId,
        email: &str,
        password: &str,
        cpf_raw: &str,
        request_ip: Option<&str>,
    ) -> Result<()> {
        self.request_common(
            org,
            email,
            password,
            cpf_raw,
            PendingRole::Cidadao,
            None,
            request_ip,
        )
        .await
    }

    /// Passo 1b — político(a). Antes de qualquer coisa, valida que
    /// `email == mandate.public_email` (o único proof automático de controle
    /// do mandato). Erro opaco em caso de mismatch — mesma lógica do
    /// register_politician atual, pra não vazar enumeração de mandatos.
    ///
    /// # Errors
    /// Idem [`Self::request_cidadao`] + [`Error::Conflict`] quando o mandato
    /// não existe ou o e-mail não confere.
    pub async fn request_politico(
        &self,
        org: OrgId,
        email: &str,
        password: &str,
        cpf_raw: &str,
        mandate_id: Uuid,
        request_ip: Option<&str>,
    ) -> Result<()> {
        let email_lc = normalize_email(email)?;
        let mandate_email = queries::find_mandate_public_email(&self.db, org.as_uuid(), mandate_id)
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| {
                Error::Conflict("Mandato não encontrado ou e-mail não confere.".to_owned())
            })?;
        if mandate_email.to_lowercase() != email_lc {
            return Err(Error::Conflict(
                "Mandato não encontrado ou e-mail não confere.".to_owned(),
            ));
        }
        self.request_common(
            org,
            email,
            password,
            cpf_raw,
            PendingRole::Politico,
            Some(mandate_id),
            request_ip,
        )
        .await
    }

    async fn request_common(
        &self,
        org: OrgId,
        email: &str,
        password: &str,
        cpf_raw: &str,
        role: PendingRole,
        mandate_id: Option<Uuid>,
        request_ip: Option<&str>,
    ) -> Result<()> {
        let now = self.clock.now();
        let email = normalize_email(email)?;
        let cpf = Cpf::parse(cpf_raw)?;
        // Trava mínima de senha aqui — bater com validação do register atual.
        if password.len() < 8 {
            return Err(Error::Validation(
                "senha deve ter ao menos 8 caracteres".to_owned(),
            ));
        }
        let password_hash = hash_password(password)?;
        let token = generate_token();
        let token_hash = sha256(&token);
        let expires_at = now + chrono::Duration::seconds(self.ttl_secs);

        // Mesma UX do password_reset: novo link mata o antigo pro mesmo e-mail.
        queries::pending_signup_invalidate_live_for_email(
            &self.db,
            org.as_uuid(),
            &email,
            now,
        )
        .await
        .map_err(map_sqlx)?;

        queries::pending_signup_insert(
            &self.db,
            Uuid::now_v7(),
            org.as_uuid(),
            &email,
            &password_hash,
            cpf.as_str(),
            role.as_str(),
            mandate_id,
            &token_hash,
            expires_at,
            request_ip,
            now,
        )
        .await
        .map_err(map_sqlx)?;

        let url = format!("{}/confirmar-conta?token={}", self.public_origin, token);
        self.deliver_email(&email, &url).await;
        Ok(())
    }

    /// Passo 2 — redime o token e materializa a conta. Retorna a sessão pronta
    /// pra o handler HTTP setar o cookie e o front redirecionar como se fosse
    /// login normal.
    ///
    /// # Errors
    /// [`Error::Unauthorized`] pra token inválido/expirado/já usado
    /// (deliberadamente opaco);
    /// [`Error::Conflict`] se e-mail ou CPF já foram levados por outro cadastro
    /// entre o request e o confirm (mesma mensagem do register atual);
    /// [`Error::Storage`] em falha dura.
    pub async fn confirm(&self, token: &str) -> Result<IssuedSession> {
        let hash = sha256(token);
        let now = self.clock.now();
        let row = queries::pending_signup_find_live(&self.db, &hash, now)
            .await
            .map_err(map_sqlx)?
            .ok_or(Error::Unauthorized)?;

        let role = PendingRole::parse(&row.role).ok_or_else(|| {
            Error::Storage(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown pending_signup.role: {}", row.role),
            )))
        })?;
        let cpf = Cpf::parse(&row.cpf)?;
        let cpf_status = self.cpf_verifier.verify(&cpf).await;
        let level = match role {
            PendingRole::Politico => VerificationLevel::Directory,
            PendingRole::Cidadao => match cpf_status {
                crate::credential::CpfStatus::Verified => VerificationLevel::Strong,
                _ => VerificationLevel::Email,
            },
        };
        let org = OrgId::from_uuid(row.org_id);

        let mut tx = self.db.begin().await.map_err(map_sqlx)?;
        // Mark used PRIMEIRO. Se algo mais adiante falhar, a tx roll-back e o
        // token permanece redimível — usuário pode tentar de novo pelo mesmo link.
        queries::pending_signup_mark_used(&mut *tx, row.id, now)
            .await
            .map_err(map_sqlx)?;

        let citizen = CitizenId::new();
        queries::insert_credential_citizen(
            &mut *tx,
            citizen.as_uuid(),
            org.as_uuid(),
            domain::level_as_str(level),
            now,
        )
        .await
        .map_err(map_register_sqlx)?;
        queries::insert_credential(
            &mut *tx,
            Uuid::now_v7(),
            citizen.as_uuid(),
            org.as_uuid(),
            &row.email,
            &row.password_hash,
            cpf.as_str(),
            cpf_status.as_str(),
            now,
        )
        .await
        .map_err(map_register_sqlx)?;

        if role == PendingRole::Politico {
            // is_public já defaulta a true na migration 0106, mas político é
            // ir além: transparência é NÃO-opt-out — força explícito.
            queries::force_citizen_public(&mut *tx, citizen.as_uuid())
                .await
                .map_err(map_sqlx)?;
            if let Some(mandate_id) = row.mandate_id {
                queries::insert_mandate_identity_binding(
                    &mut *tx,
                    Uuid::now_v7(),
                    mandate_id,
                    citizen.as_uuid(),
                    "directory",
                    None,
                    now,
                )
                .await
                .map_err(map_sqlx)?;
            }
        }

        let session = issue_session(
            &mut tx,
            org,
            citizen,
            &row.email,
            now,
            self.session_ttl_secs,
        )
        .await?;
        tx.commit().await.map_err(map_sqlx)?;
        Ok(session)
    }

    async fn deliver_email(&self, to: &str, confirm_url: &str) {
        let Some(smtp) = &self.smtp else {
            tracing::info!(
                target: "auth::signup_verify",
                to,
                confirm_url,
                "DEV: SMTP unconfigured; signup-verify URL logged instead of sent."
            );
            return;
        };
        let subject = "DemocraciaBR — confirme sua conta";
        let body = format!(
            "Olá,\n\nRecebemos seu cadastro na DemocraciaBR. Pra ativar a conta \
             e fazer o primeiro login, abra este link em até 24 horas:\n\n{confirm_url}\n\n\
             Se não foi você quem se cadastrou, é só ignorar esta mensagem — \
             a conta nunca é criada sem esta confirmação.\n\n— DemocraciaBR"
        );
        let to_owned = to.to_owned();
        let smtp = smtp.clone();
        let subject = subject.to_owned();
        // Non-blocking: a request retorna mesmo se o relay travar. Falha de
        // envio só é auditada — mesma escolha do password_reset.
        tokio::spawn(async move {
            if let Err(err) = send_email(&smtp, &to_owned, &subject, &body).await {
                tracing::error!(error = ?err, "signup-verify e-mail send failed");
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Helpers puros
// ---------------------------------------------------------------------------

fn normalize_email(email: &str) -> Result<String> {
    let e = email.trim().to_lowercase();
    if e.len() < 3 || !e.contains('@') || !e.split('@').nth(1).is_some_and(|d| d.contains('.')) {
        return Err(Error::Validation("e-mail inválido".to_string()));
    }
    Ok(e)
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

async fn issue_session(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    org: OrgId,
    citizen: CitizenId,
    email: &str,
    now: DateTime<Utc>,
    session_ttl_secs: i64,
) -> Result<IssuedSession> {
    let subject = format!("cred:{email}");
    let handle = domain::public_handle(citizen);
    let session_id = Uuid::now_v7();
    let expires_at = domain::compute_expiry(now, session_ttl_secs);
    queries::insert_session(
        &mut **tx,
        session_id,
        org.as_uuid(),
        citizen.as_uuid(),
        &subject,
        now,
        expires_at,
        &handle,
    )
    .await
    .map_err(map_sqlx)?;
    Ok(IssuedSession {
        id: session_id,
        citizen,
        oidc_subject: subject,
        issued_at: now,
        expires_at,
        public_handle: handle,
    })
}

fn generate_token() -> String {
    let mut buf = [0u8; TOKEN_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

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

/// Espelha `crate::service::map_register_sqlx`: unique-violation em email/cpf
/// no confirm vira Conflict com mensagem específica ("email_taken" / "cpf_taken").
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_are_distinct_and_url_safe() {
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b);
        assert_eq!(a.len(), 43); // 256 bits em base64url no-pad
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn sha256_is_deterministic_and_distinct() {
        assert_eq!(sha256("abc"), sha256("abc"));
        assert_ne!(sha256("abc"), sha256("abd"));
        assert_eq!(sha256("").len(), 32);
    }

    #[test]
    fn role_roundtrip() {
        for r in [PendingRole::Cidadao, PendingRole::Politico] {
            assert_eq!(PendingRole::parse(r.as_str()), Some(r));
        }
        assert_eq!(PendingRole::parse("outro"), None);
    }

    #[test]
    fn normalize_email_lowercases_and_validates() {
        assert_eq!(normalize_email("  Foo@Bar.com ").unwrap(), "foo@bar.com");
        assert!(normalize_email("no-at").is_err());
        assert!(normalize_email("a@b").is_err()); // sem ponto no domínio
    }
}

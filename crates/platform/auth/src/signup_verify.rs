//! # Signup verify — e-mail verification before the account exists (0.25.0-fediverse).
//!
//! Old flow: `POST /auth/register` wrote citizen + credential + session
//! immediately. That meant any bot with a valid identity document created a
//! live account without proving control of the e-mail, and a document stayed
//! locked by wrong/invalid e-mails.
//!
//! New flow (`ADR-0011`):
//! 1. **Request** (`POST /auth/register` or `.../politician`): we normalize,
//!    validate and hash everything, and persist an `auth_pending_signup`
//!    (migration 0106) with a SHA-256 token. **No row in `citizen`**
//!    yet. An e-mail with the link `<origin>/confirmar-conta?token=…` goes out
//!    through the existing SMTP infrastructure (the password-reset transport).
//! 2. **Confirm** (`POST /auth/register/confirm`): recebemos o plaintext do
//!    link, find the pending row by hash, and materialize citizen, credential
//!    and session in a single transaction (mark used, insert citizen, insert
//!    credential, [politico: mandate_binding + is_public=true], issue session).
//!
//! Mirrors the [`crate::password_reset`] pattern — same hashing, same short
//! TTL, same dev-mode care of logging the link when SMTP is unconfigured.

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

/// Default TTL of the verification link (24h). Override with `AUTH_SIGNUP_VERIFY_TTL_SECS`.
const DEFAULT_TTL_SECS: i64 = 24 * 3600;
/// Random bytes in the token (256 bits → 43 base64url chars, no padding). Same size as password_reset.
const TOKEN_BYTES: usize = 32;
/// SMTP send timeout — never stall the request when the relay is slow.
const SMTP_TIMEOUT_SECS: u64 = 5;
/// Window and cap of the per-IP rate limit on `request` (P3.1). Configurable via
/// `AUTH_SIGNUP_RATE_MAX_PER_HOUR` (default 3). No override when
/// `request_ip=None` — a caller without X-Forwarded-For never locks anyone out.
const DEFAULT_RATE_MAX_PER_HOUR: i64 = 3;

/// Role of the pending signup. Determines which materialization `confirm`
/// executa (register vs register_politician).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingRole {
    Cidadao,
    Politico,
    Candidato,
}

impl PendingRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cidadao => "cidadao",
            Self::Politico => "politico",
            Self::Candidato => "candidato",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "cidadao" => Some(Self::Cidadao),
            "politico" => Some(Self::Politico),
            "candidato" => Some(Self::Candidato),
            _ => None,
        }
    }
}

/// Target election of a candidate signup. When 2028 arrives, this becomes env/config.
const CANDIDATE_ELECTION_YEAR: i32 = 2026;

/// Metadados da candidatura auto-declarada (migration 0526). Validados no
/// request ([`CandidateMeta::validated`]), persistidos como jsonb na pending
/// e materializados no confirm (mandate + binding + candidacy).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CandidateMeta {
    /// Ballot name (public).
    pub display_name: String,
    /// Cargo pretendido — mesmo eixo de `candidacy.office`.
    pub office: String,
    /// UF (mandatory except for president).
    pub uf: Option<String>,
    /// Municipality (mandatory for municipal offices).
    pub municipio: Option<String>,
    /// Party sigla of the current affiliation.
    pub party_sigla: String,
    /// Ballot number, when the party has already assigned one.
    pub number: Option<String>,
}

/// Esfera derivada do cargo — mesma taxonomia de `candidacy.office`/`election.sphere`.
fn office_sphere(office: &str) -> Option<&'static str> {
    match office {
        "presidente" | "senador" | "deputado_federal" => Some("federal"),
        "governador" | "deputado_estadual" => Some("estadual"),
        "prefeito" | "vice_prefeito" | "vereador" => Some("municipal"),
        _ => None,
    }
}

impl CandidateMeta {
    /// Normaliza + valida. Retorna `(meta_normalizada, sphere)`.
    ///
    /// # Errors
    /// [`Error::Validation`] with a friendly message per invalid field.
    pub fn validated(mut self) -> Result<(Self, &'static str)> {
        self.display_name = self.display_name.trim().to_owned();
        if self.display_name.chars().count() < 3 || self.display_name.chars().count() > 80 {
            return Err(Error::Validation(
                "nome de urna deve ter entre 3 e 80 caracteres".to_owned(),
            ));
        }
        let sphere = office_sphere(&self.office).ok_or_else(|| {
            Error::Validation(
                "cargo inválido — use presidente, governador, senador, deputado_federal, \
                 deputado_estadual, prefeito, vice_prefeito ou vereador"
                    .to_owned(),
            )
        })?;
        self.uf = match self.uf.take() {
            Some(uf) => {
                let uf = uf.trim().to_uppercase();
                if uf.len() != 2 || !uf.chars().all(|c| c.is_ascii_alphabetic()) {
                    return Err(Error::Validation("UF inválida (2 letras)".to_owned()));
                }
                Some(uf)
            }
            None => None,
        };
        if self.office != "presidente" && self.uf.is_none() {
            return Err(Error::Validation(
                "UF é obrigatória pra esse cargo".to_owned(),
            ));
        }
        self.municipio = self
            .municipio
            .take()
            .map(|m| m.trim().to_owned())
            .filter(|m| !m.is_empty());
        if sphere == "municipal" && self.municipio.is_none() {
            return Err(Error::Validation(
                "município é obrigatório pra cargos municipais".to_owned(),
            ));
        }
        if let Some(m) = &self.municipio {
            if m.chars().count() > 120 {
                return Err(Error::Validation("município longo demais".to_owned()));
            }
        }
        self.party_sigla = self.party_sigla.trim().to_uppercase();
        if self.party_sigla.chars().count() < 2 || self.party_sigla.chars().count() > 20 {
            return Err(Error::Validation(
                "sigla do partido deve ter entre 2 e 20 caracteres".to_owned(),
            ));
        }
        self.number = self
            .number
            .take()
            .map(|n| n.trim().to_owned())
            .filter(|n| !n.is_empty());
        if let Some(n) = &self.number {
            if n.len() < 2 || n.len() > 5 || !n.chars().all(|c| c.is_ascii_digit()) {
                return Err(Error::Validation(
                    "número de urna deve ter de 2 a 5 dígitos".to_owned(),
                ));
            }
        }
        Ok((self, sphere))
    }
}

/// Result of `confirm`: a ready session (open instance) or an account created
/// but held for manual review (`GATEWAY_SIGNUP_REQUIRES_REVIEW`, 0514).
#[derive(Debug)]
pub enum ConfirmOutcome {
    Session(Box<IssuedSession>),
    PendingReview { email: String },
}

/// Does the instance require manual approval of new signups? The env var name
/// segue o documentado na migration 0514 (`GATEWAY_SIGNUP_REQUIRES_REVIEW`).
fn signup_requires_review() -> bool {
    std::env::var("GATEWAY_SIGNUP_REQUIRES_REVIEW")
        .map(|v| matches!(v.trim(), "1" | "true" | "TRUE" | "yes" | "on"))
        .unwrap_or(false)
}

#[derive(Clone)]
struct SmtpConfig {
    host: String,
    port: u16,
    user: Option<String>,
    pass: Option<String>,
    from: String,
}

/// Service for e-mail verification at signup. Follows the same shape as
/// [`crate::password_reset::PasswordResetService`] — construction via
/// [`Self::from_state`] to read SMTP + the public origin from the environment.
#[derive(Clone)]
pub struct SignupVerifyService {
    db: Db,
    clock: std::sync::Arc<dyn dsoc_core::Clock>,
    public_origin: String,
    smtp: Option<SmtpConfig>,
    ttl_secs: i64,
    session_ttl_secs: i64,
    cpf_verifier: std::sync::Arc<dyn CpfVerifier>,
    identity_verifier: std::sync::Arc<dyn crate::identity_verify::IdentityVerifier>,
}

/// Self-declared identity data at signup, matched against the authorized base
/// (R-KYC #50). All optional for compat; the new front end sends
/// name+birth+sex (verification) and, optionally, the electoral registry.
#[derive(Debug, Clone, Default)]
pub struct SignupIdentity {
    /// Nome completo informado.
    pub nome_completo: Option<String>,
    /// Data de nascimento `YYYY-MM-DD`.
    pub nascimento: Option<String>,
    /// Sexo `M`/`F`.
    pub sexo: Option<String>,
    /// Electoral registry (optional; without it = no voting power).
    pub titulo_eleitor: Option<String>,
    /// Residence UF (2-letter code). Mandatory for a citizen (the territorial axis).
    pub uf: Option<String>,
    /// Residence municipality (IBGE code). Mandatory for a citizen; must exist
    /// in `municipio_ibge` and belong to the `uf`.
    pub municipio_ibge: Option<i32>,
    /// Chosen fediverse nick (handle). Mandatory for a citizen (0664).
    pub handle: Option<String>,
}

/// Normalize+validate a fediverse nick: lowercase, `[a-z0-9_]`, 3–30 chars.
/// Returns the normalized handle or a validation error. Used at signup (0664).
pub fn normalize_handle(raw: &str) -> Result<String> {
    let h = raw.trim().trim_start_matches('@').to_lowercase();
    let ok = (3..=30).contains(&h.chars().count())
        && h.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && h.chars().next().is_some_and(|c| c.is_ascii_lowercase());
    if ok {
        Ok(h)
    } else {
        Err(Error::Validation(
            "nick inválido: use 3 a 30 caracteres — letras minúsculas, números ou _, começando por letra"
                .to_owned(),
        ))
    }
}

/// ASCII slug of a name to derive a handle (B4). Lowercase, common Portuguese
/// diacritics folded to ASCII, separators (space/punctuation) become `_` and the
/// rest is dropped. E.g. "José da Silva" → "jose_da_silva".
fn slugify_name(name: &str) -> String {
    let mut out = String::new();
    let mut pending_sep = false;
    for c in name.trim().to_lowercase().chars() {
        let mapped = match c {
            'a'..='z' | '0'..='9' => Some(c),
            'á' | 'à' | 'â' | 'ã' | 'ä' | 'å' => Some('a'),
            'é' | 'è' | 'ê' | 'ë' => Some('e'),
            'í' | 'ì' | 'î' | 'ï' => Some('i'),
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => Some('o'),
            'ú' | 'ù' | 'û' | 'ü' => Some('u'),
            'ç' => Some('c'),
            'ñ' => Some('n'),
            _ => None,
        };
        match mapped {
            Some(ch) => {
                if pending_sep && !out.is_empty() {
                    out.push('_');
                }
                pending_sep = false;
                out.push(ch);
            }
            None => pending_sep = true,
        }
    }
    out
}

/// Map the form's sex (`F`/`M`) onto the `citizen.gender` vocabulary.
fn sexo_to_gender(sexo: &str) -> Option<&'static str> {
    match sexo.trim().to_uppercase().as_str() {
        "F" => Some("mulher"),
        "M" => Some("homem"),
        _ => None,
    }
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
    /// Build the service explicitly — useful for integration tests that inject
    /// `Db` + `Arc<dyn Clock>` directly (bypassing `AppState` and env vars).
    /// Always in dev-mode (SMTP=None) — the e-mail is logged,
    /// never sent.
    #[must_use]
    pub fn new_for_tests(
        db: Db,
        clock: std::sync::Arc<dyn dsoc_core::Clock>,
        public_origin: impl Into<String>,
        ttl_secs: i64,
        session_ttl_secs: i64,
    ) -> Self {
        Self {
            db,
            clock,
            public_origin: public_origin.into(),
            smtp: None,
            ttl_secs,
            session_ttl_secs,
            cpf_verifier: std::sync::Arc::new(AlgorithmicCpfVerifier),
            // Tests have no external service: Noop (skipped → signup proceeds).
            identity_verifier: std::sync::Arc::new(crate::identity_verify::NoopIdentityVerifier),
        }
    }

    /// Build from `AppState` + env. Recognized env vars:
    /// - `PUBLIC_ORIGIN` (default `https://democracia.social.br`)
    /// - `SMTP_HOST` / `SMTP_PORT` / `SMTP_USER` / `SMTP_PASS` / `SMTP_FROM`
    /// - `AUTH_SIGNUP_VERIFY_TTL_SECS` (default 86400)
    /// - `AUTH_SESSION_TTL_SECS` (default `DEFAULT_SESSION_TTL_SECS`, used when confirm issues a session)
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
            // HTTP when CPF_VERIFY_URL is set, otherwise Noop (graceful degradation).
            identity_verifier: crate::identity_verify::from_env(),
        }
    }

    /// Step 1a — ordinary citizen. Normalizes + validates the document + hashes the
    /// password + mints the token + persists the pending row + fires the e-mail. Always
    /// succeeds on the wire (the password_reset enumeration resistance): someone who
    /// supplied an existing e-mail learns that through *another* path — the
    /// unique violation only happens at confirm.
    ///
    /// # Errors
    /// [`Error::Validation`] for an invalid e-mail/password/document;
    /// [`Error::Storage`] on a hard persistence failure.
    pub async fn request_cidadao(
        &self,
        org: OrgId,
        email: &str,
        password: &str,
        cpf_raw: &str,
        identity: SignupIdentity,
        request_ip: Option<&str>,
    ) -> Result<()> {
        self.request_common(
            org,
            email,
            password,
            cpf_raw,
            PendingRole::Cidadao,
            None,
            None,
            identity,
            request_ip,
        )
        .await
    }

    /// Step 1c — candidate WITHOUT a mandate (0526). No automatic proof is
    /// possible (the person appears in no official registry yet), so the
    /// metadata is validated and stored as a self-declaration; confirm
    /// materializes a mandate with `source='self'` + an `email`-level binding +
    /// a `listed=false` candidacy. Verification (party/TSE/admin) comes later.
    ///
    /// # Errors
    /// Idem [`Self::request_cidadao`] + [`Error::Validation`] pros campos da
    /// candidatura.
    pub async fn request_candidato(
        &self,
        org: OrgId,
        email: &str,
        password: &str,
        cpf_raw: &str,
        meta: CandidateMeta,
        request_ip: Option<&str>,
    ) -> Result<()> {
        let (meta, _sphere) = meta.validated()?;
        let meta_json = serde_json::to_value(&meta).map_err(|e| {
            Error::Storage(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                e,
            )))
        })?;
        self.request_common(
            org,
            email,
            password,
            cpf_raw,
            PendingRole::Candidato,
            None,
            Some(meta_json),
            SignupIdentity::default(),
            request_ip,
        )
        .await
    }

    /// Step 1b — politician. Before anything else, validates that
    /// `email == mandate.public_email` (the only automatic proof of control over
    /// the mandate). An opaque error on mismatch — the same logic as the current
    /// register_politician, so mandate enumeration never leaks.
    ///
    /// # Errors
    /// Same as [`Self::request_cidadao`] + [`Error::Conflict`] when the mandate
    /// does not exist or the e-mail does not match.
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
            None,
            SignupIdentity::default(),
            request_ip,
        )
        .await
    }

    // 8 args: the signup pipeline is a linear sequence of validated data;
    // grouping them into a struct would add a layer with no extra callers (2 sites).
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    async fn request_common(
        &self,
        org: OrgId,
        email: &str,
        password: &str,
        cpf_raw: &str,
        role: PendingRole,
        mandate_id: Option<Uuid>,
        candidate_meta: Option<serde_json::Value>,
        identity: SignupIdentity,
        request_ip: Option<&str>,
    ) -> Result<()> {
        let now = self.clock.now();
        // Per-IP rate limit BEFORE anything else — cheap (one query) and it avoids
        // gastar Argon2 em cima de flood.
        if let Some(ip) = request_ip {
            let since = now - chrono::Duration::hours(1);
            let count = queries::pending_signup_count_by_ip_since(&self.db, ip, since)
                .await
                .map_err(map_sqlx)?;
            let max = std::env::var("AUTH_SIGNUP_RATE_MAX_PER_HOUR")
                .ok()
                .and_then(|v| v.parse().ok())
                .filter(|&v: &i64| v > 0)
                .unwrap_or(DEFAULT_RATE_MAX_PER_HOUR);
            if count >= max {
                return Err(Error::RateLimit(
                    "muitas tentativas de cadastro deste IP na última hora".to_owned(),
                ));
            }
        }
        let email = normalize_email(email)?;
        let cpf = Cpf::parse(cpf_raw)?;

        // R-KYC #50: when name+birth+sex arrive, match them against the authorized
        // base via the cpf-verify SaaS. REJECT blocks the signup; the other bands
        // (ACCEPT/REVIEW/ESCALATE) and Skipped (service absent/down — fail-open)
        // proceed. Human review of the REVIEW/ESCALATE bands is handled in the
        // persistence slice (part 2); here we only block the clear negative.
        if let (Some(nome), true) = (
            identity
                .nome_completo
                .as_deref()
                .filter(|s| !s.trim().is_empty()),
            identity.nascimento.is_some() || identity.sexo.is_some(),
        ) {
            let query = crate::identity_verify::IdentityQuery {
                cpf: cpf.as_str().to_owned(),
                nome: nome.to_owned(),
                nascimento: identity.nascimento.clone(),
                sexo: identity.sexo.clone(),
            };
            let verdict = self.identity_verifier.verify_identity(&query).await;
            if !verdict.allows_registration() {
                tracing::info!(
                    faixa = verdict.faixa().as_str(),
                    "cadastro barrado na verificação"
                );
                return Err(Error::Validation(
                    "não conseguimos confirmar seus dados (nome, data de nascimento ou sexo) \
                     com o CPF informado. Confira as informações e tente novamente."
                        .to_owned(),
                ));
            }
        }

        // Residence mandatory for a citizen (0651/0652/0653): UF + IBGE municipality,
        // and the municipality must exist and belong to the UF. Politicians/candidates
        // do not declare residence here — their territory comes from the mandate/candidacy.
        let (residencia_uf, residencia_municipio) = if role == PendingRole::Cidadao {
            let uf = identity
                .uf
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_uppercase);
            let (Some(uf), Some(codigo)) = (uf, identity.municipio_ibge) else {
                return Err(Error::Validation(
                    "informe seu estado e município de domicílio".to_owned(),
                ));
            };
            if !queries::municipio_belongs_to_uf(&self.db, codigo, &uf)
                .await
                .map_err(map_sqlx)?
            {
                return Err(Error::Validation(
                    "estado ou município de domicílio inválidos".to_owned(),
                ));
            }
            (Some(uf), Some(codigo))
        } else {
            (None, None)
        };

        // The citizen's personal data. MANDATORY at signup: name (with a surname)
        // and birth date (the latter feeds R-KYC verification alongside the document).
        // OPTIONAL (B4 — lean onboarding): sex (ProfileGate collects it later) and the
        // fediverse nick (derived from the name; editable in Settings).
        // Politicians/candidates inherit name/territory from the mandate/candidacy → None.
        let (full_name, gender, birth_date, handle) = if role == PendingRole::Cidadao {
            let Some(nome) = identity
                .nome_completo
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                return Err(Error::Validation("informe seu nome completo".to_owned()));
            };
            if nome.split_whitespace().count() < 2 {
                return Err(Error::Validation("informe nome e sobrenome".to_owned()));
            }
            // B4: sex is OPTIONAL. Absent stays None; ProfileGate collects it
            // later (profile-status already lists "sexo" as missing).
            let gender = identity.sexo.as_deref().and_then(sexo_to_gender);
            let Some(birth_raw) = identity
                .nascimento
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                return Err(Error::Validation(
                    "informe sua data de nascimento".to_owned(),
                ));
            };
            let birth_date = chrono::NaiveDate::parse_from_str(birth_raw, "%Y-%m-%d")
                .map_err(|_| Error::Validation("data de nascimento inválida".to_owned()))?;
            // B4: the nick is no longer asked at signup. If the citizen supplied one
            // (advanced/future flow) we honour and validate it; otherwise we derive it
            // from the name. handle=None makes confirm() fall back to `cidadao-<id8>`.
            let handle = match identity
                .handle
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                Some(raw) => {
                    let h = normalize_handle(raw)?;
                    if !queries::handle_available(&self.db, org.as_uuid(), &h)
                        .await
                        .map_err(map_sqlx)?
                    {
                        return Err(Error::Validation(
                            "esse nick já está em uso — escolha outro".to_owned(),
                        ));
                    }
                    Some(h)
                }
                None => self.derive_unique_handle(org.as_uuid(), nome).await?,
            };
            (
                Some(nome.to_owned()),
                gender.map(str::to_owned),
                Some(birth_date),
                handle,
            )
        } else {
            (None, None, None, None)
        };

        // Minimal password floor here — matches the current register validation.
        if password.len() < 8 {
            return Err(Error::Validation(
                "senha deve ter ao menos 8 caracteres".to_owned(),
            ));
        }
        let password_hash = hash_password(password)?;
        let token = generate_token();
        let token_hash = sha256(&token);
        let expires_at = now + chrono::Duration::seconds(self.ttl_secs);

        // Same UX as password_reset: a new link kills the previous one for that e-mail.
        queries::pending_signup_invalidate_live_for_email(&self.db, org.as_uuid(), &email, now)
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
            candidate_meta.as_ref(),
            residencia_uf.as_deref(),
            residencia_municipio,
            &token_hash,
            expires_at,
            request_ip,
            now,
            full_name.as_deref(),
            gender.as_deref(),
            birth_date,
            handle.as_deref(),
        )
        .await
        .map_err(map_sqlx)?;

        let url = format!("{}/confirmar-conta?token={}", self.public_origin, token);
        self.deliver_email(&email, &url).await;
        Ok(())
    }

    /// Derive a unique handle from the name (B4 — the nick is no longer asked at
    /// signup). ASCII-slugs the name, guarantees a letter as the first character and
    /// 3–30 chars, and resolves collisions with a numeric suffix. Returns `None` when
    /// there is no usable base (a name of symbols only) or every variant is already
    /// taken — in that case `confirm()` generates the automatic `cidadao-<id8>`.
    async fn derive_unique_handle(&self, org: Uuid, name: &str) -> Result<Option<String>> {
        let mut base = slugify_name(name);
        // The first character must be a letter (handle rule).
        if !base.chars().next().is_some_and(|c| c.is_ascii_lowercase()) {
            base.insert(0, 'c');
        }
        // Leave room for a numeric suffix within the 30-char cap. The base is pure
        // ASCII (slugify), so truncating by bytes is safe.
        base.truncate(26);
        while base.ends_with('_') {
            base.pop();
        }
        if base.len() < 3 {
            return Ok(None);
        }
        // Try the bare base, then base2..=base9. confirm() re-checks availability
        // and falls back to the automatic handle if it loses the request→confirm race.
        for suffix in std::iter::once(String::new()).chain((2..=9).map(|n| n.to_string())) {
            let candidate = format!("{base}{suffix}");
            if queries::handle_available(&self.db, org, &candidate)
                .await
                .map_err(map_sqlx)?
            {
                return Ok(Some(candidate));
            }
        }
        Ok(None)
    }

    /// Resend the verification link to an e-mail with a live pending row. When
    /// there is no live pending, it silently does nothing — the wire response is
    /// 200 either way (enumeration-safe, the password_reset pattern).
    ///
    /// Reaproveita password_hash+cpf+role+mandate_id da pending existente
    /// (the user types nothing new). Mints a fresh token, invalidates the previous
    /// pending row, inserts a new one, fires the e-mail.
    ///
    /// # Errors
    /// [`Error::Storage`] on a hard persistence failure.
    pub async fn resend(&self, org: OrgId, email: &str, request_ip: Option<&str>) -> Result<()> {
        let now = self.clock.now();
        let email = match normalize_email(email) {
            Ok(e) => e,
            Err(_) => return Ok(()), // silêncio pra e-mail obviamente inválido
        };
        let Some(row) =
            queries::pending_signup_find_live_for_email(&self.db, org.as_uuid(), &email, now)
                .await
                .map_err(map_sqlx)?
        else {
            return Ok(()); // nada pra reenviar
        };
        let role = PendingRole::parse(&row.role).ok_or_else(|| {
            Error::Storage(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown pending_signup.role: {}", row.role),
            )))
        })?;
        let token = generate_token();
        let token_hash = sha256(&token);
        let expires_at = now + chrono::Duration::seconds(self.ttl_secs);

        queries::pending_signup_invalidate_live_for_email(&self.db, org.as_uuid(), &email, now)
            .await
            .map_err(map_sqlx)?;
        queries::pending_signup_insert(
            &self.db,
            Uuid::now_v7(),
            org.as_uuid(),
            &email,
            &row.password_hash,
            &row.cpf,
            role.as_str(),
            row.mandate_id,
            row.candidate_meta.as_ref(),
            row.residencia_uf.as_deref(),
            row.residencia_municipio_ibge,
            &token_hash,
            expires_at,
            request_ip,
            now,
            row.full_name.as_deref(),
            row.gender.as_deref(),
            row.birth_date,
            row.handle.as_deref(),
        )
        .await
        .map_err(map_sqlx)?;

        let url = format!("{}/confirmar-conta?token={}", self.public_origin, token);
        self.deliver_email(&email, &url).await;
        Ok(())
    }

    /// Cleanup worker (P3.3): removes pendings that expired more than
    /// `cutoff_days` days ago. Returns how many were deleted — useful for metrics.
    ///
    /// # Errors
    /// [`Error::Storage`] em falha de DELETE.
    pub async fn cleanup_expired(&self, cutoff_days: i64) -> Result<u64> {
        let cutoff = self.clock.now() - chrono::Duration::days(cutoff_days.max(1));
        queries::pending_signup_cleanup_expired(&self.db, cutoff)
            .await
            .map_err(map_sqlx)
    }

    /// Cleanup of `auth_login_attempt` (P5.1). Not part of the signup service
    /// itself, but we run it in the same worker to save loops. Uses the same
    /// cutoff_days for operational consistency.
    ///
    /// # Errors
    /// [`Error::Storage`] em falha de DELETE.
    pub async fn cleanup_login_attempts_via(
        state: &dsoc_app::AppState,
        cutoff_days: i64,
    ) -> Result<u64> {
        let cutoff = state.clock.now() - chrono::Duration::days(cutoff_days.max(1));
        queries::login_attempt_cleanup(&state.db, cutoff)
            .await
            .map_err(map_sqlx)
    }

    /// Step 2 — redeem the token and materialize the account. On an open instance
    /// it returns the session ready for the HTTP handler to set the cookie; when
    /// `GATEWAY_SIGNUP_REQUIRES_REVIEW` is on (migration 0514), the account is
    /// born with `pending_review = true`, NO session is issued and the outcome
    /// tells the front end that admin approval is missing (/admin/revisoes).
    ///
    /// # Errors
    /// [`Error::Unauthorized`] for an invalid/expired/already-used token
    /// (deliberadamente opaco);
    /// [`Error::Conflict`] when the e-mail or document was taken by another signup
    /// entre o request e o confirm (mesma mensagem do register atual);
    /// [`Error::Storage`] em falha dura.
    pub async fn confirm(&self, token: &str) -> Result<ConfirmOutcome> {
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
            // A self-declared candidate does NOT get 'directory' — there is no official
            // record to check against. They climb via attestation/TSE/admin later.
            PendingRole::Cidadao | PendingRole::Candidato => match cpf_status {
                crate::credential::CpfStatus::Verified => VerificationLevel::Strong,
                _ => VerificationLevel::Email,
            },
        };
        let org = OrgId::from_uuid(row.org_id);

        let mut tx = self.db.begin().await.map_err(map_sqlx)?;
        // Mark used PRIMEIRO. Se algo mais adiante falhar, a tx roll-back e o
        // the token stays redeemable — the user can retry with the same link.
        queries::pending_signup_mark_used(&mut *tx, row.id, now)
            .await
            .map_err(map_sqlx)?;

        let citizen = CitizenId::new();
        // Dados pessoais do cadastro (0664): nome → display_name + legal_name (o nome
        // the given civil name is the one shown), sex → gender, birth → birth_date.
        queries::insert_credential_citizen(
            &mut *tx,
            citizen.as_uuid(),
            org.as_uuid(),
            domain::level_as_str(level),
            row.residencia_uf.as_deref(),
            row.residencia_municipio_ibge,
            now,
            row.full_name.as_deref(),
            row.full_name.as_deref(),
            row.gender.as_deref(),
            row.birth_date,
        )
        .await
        .map_err(map_register_sqlx)?;

        // Fediverse handle: uses the NICK chosen at signup (0664) when still free;
        // otherwise falls back to the automatic `cidadao-<id8>` (unique by construction).
        // Politicians/candidates (no nick at signup) also fall back — they change it later.
        let chosen = match row.handle.as_deref() {
            Some(h)
                if queries::handle_available(&mut *tx, org.as_uuid(), h)
                    .await
                    .map_err(map_sqlx)? =>
            {
                h.to_owned()
            }
            _ => format!(
                "cidadao-{}",
                &citizen.as_uuid().as_simple().to_string()[..8]
            ),
        };
        queries::set_handle_if_null(&mut *tx, citizen.as_uuid(), &chosen)
            .await
            .map_err(map_sqlx)?;
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
            // is_public already defaults to true in migration 0106, but a politician
            // goes further: transparency is NOT opt-out — force it explicitly.
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

        if role == PendingRole::Candidato {
            // Same non-opt-out transparency as the politician: a candidacy is public.
            queries::force_citizen_public(&mut *tx, citizen.as_uuid())
                .await
                .map_err(map_sqlx)?;
            let meta: CandidateMeta = row
                .candidate_meta
                .clone()
                .ok_or_else(|| {
                    Error::Storage(Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "pending role=candidato sem candidate_meta",
                    )))
                })
                .and_then(|v| {
                    serde_json::from_value(v).map_err(|e| {
                        Error::Storage(Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            e,
                        )))
                    })
                })?;
            // Defensive: re-validate what the request wrote (the database CHECK only
            // guarantees the jsonb is present, not its shape).
            let (meta, sphere) = meta.validated()?;
            let mandate_id = Uuid::now_v7();
            queries::insert_mandate_self_candidate(
                &mut *tx,
                mandate_id,
                org.as_uuid(),
                citizen.as_uuid(),
                &meta.office,
                &meta.display_name,
                &row.email,
                &meta.party_sigla,
                meta.uf.as_deref(),
                sphere,
                now,
            )
            .await
            .map_err(map_sqlx)?;
            // 'email'-level binding — the is_politico gate unlocks (panel +
            // /me/campanha), but the public badge still reads self-declared.
            queries::insert_mandate_identity_binding(
                &mut *tx,
                Uuid::now_v7(),
                mandate_id,
                citizen.as_uuid(),
                "email",
                Some(&format!("self_signup:{}", row.id)),
                now,
            )
            .await
            .map_err(map_sqlx)?;
            // Candidacy outside the comparator (listed=false) until verification.
            // With no matching election registered, it proceeds without a candidacy — the
            // mandate/binding are enough for the tooling.
            if let Some(election_id) =
                queries::find_election_id(&mut *tx, org.as_uuid(), CANDIDATE_ELECTION_YEAR, sphere)
                    .await
                    .map_err(map_sqlx)?
            {
                queries::insert_candidacy_self(
                    &mut *tx,
                    Uuid::now_v7(),
                    election_id,
                    mandate_id,
                    &meta.party_sigla,
                    &meta.office,
                    meta.number.as_deref().unwrap_or(""),
                    meta.uf.as_deref(),
                    meta.municipio.as_deref(),
                    &meta.display_name,
                )
                .await
                .map_err(map_sqlx)?;
            }
        }

        if signup_requires_review() {
            sqlx::query(r"UPDATE citizen SET pending_review = true WHERE id = $1")
                .bind(citizen.as_uuid())
                .execute(&mut *tx)
                .await
                .map_err(map_sqlx)?;
            tx.commit().await.map_err(map_sqlx)?;
            return Ok(ConfirmOutcome::PendingReview {
                email: row.email.clone(),
            });
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
        // Federated onboarding after commit, best-effort (a failure here NEVER breaks
        // the signup): actor keys (a profile resolvable on the fediverse from second
        // one) + an automatic follow of the official @socrates profile (announcements).
        self.federated_onboarding(citizen, now).await;
        // Welcome mail only after the commit — the account genuinely exists.
        self.deliver_welcome(&row.email);
        Ok(ConfirmOutcome::Session(Box::new(session)))
    }

    /// Actor keys + auto-follow of @socrates. Best-effort with logging.
    async fn federated_onboarding(&self, citizen: CitizenId, now: chrono::DateTime<chrono::Utc>) {
        match tokio::task::spawn_blocking(dsoc_federation::generate_actor_keypair).await {
            Ok(Ok(kp)) => {
                if let Err(err) = queries::insert_actor_keypair(
                    &self.db,
                    citizen.as_uuid(),
                    &kp.private_pem,
                    &kp.public_pem,
                    now,
                )
                .await
                {
                    tracing::warn!(?err, "onboarding: keypair do ator falhou");
                }
            }
            other => tracing::warn!(?other, "onboarding: geração de chave falhou"),
        }
        let origin = std::env::var("PUBLIC_ORIGIN")
            .unwrap_or_else(|_| "https://democracia.social.br".to_owned());
        let origin = origin.trim_end_matches('/');
        let socrates_actor = format!("{origin}/actors/socrates");
        let socrates_inbox = format!("{socrates_actor}/inbox");
        if let Err(err) = queries::insert_local_follow_if_absent(
            &self.db,
            Uuid::now_v7(),
            citizen.as_uuid(),
            &socrates_actor,
            &socrates_inbox,
            now,
        )
        .await
        {
            tracing::warn!(?err, "onboarding: auto-follow do socrates falhou");
        }
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
        // Template editable by the admin (0.32.0); hardcoded fallback if the row
        // vanished from the DB — the confirmation e-mail must never fail to go out.
        let mut ctx: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
        ctx.insert("confirm_url", confirm_url.to_owned());
        let (subject, body) = dsoc_db::email_templates::render(&self.db, "signup_verify", &ctx)
            .await
            .unwrap_or_else(|| {
                (
                    "DemocraciaBR — confirme sua conta".to_owned(),
                    format!(
                        "Olá,\n\nRecebemos seu cadastro na DemocraciaBR. Pra ativar a conta \
                         e fazer o primeiro login, abra este link em até 24 horas:\n\n{confirm_url}\n\n\
                         Se não foi você quem se cadastrou, é só ignorar esta mensagem — \
                         a conta nunca é criada sem esta confirmação.\n\n— DemocraciaBR"
                    ),
                )
            });
        let to_owned = to.to_owned();
        let smtp = smtp.clone();
        // Non-blocking: the request returns even if the relay stalls. A send failure
        // is only audited — the same choice as password_reset.
        tokio::spawn(async move {
            if let Err(err) = send_email(&smtp, &to_owned, &subject, &body).await {
                tracing::error!(error = ?err, "signup-verify e-mail send failed");
            }
        });
    }

    /// Post-activation welcome (0.32.0): fires after `confirm()` materializes
    /// the account. Best-effort and non-blocking — an SMTP failure never
    /// disturbs the freshly created session.
    fn deliver_welcome(&self, to: &str) {
        let Some(smtp) = &self.smtp else {
            tracing::info!(
                target: "auth::signup_verify",
                to,
                "DEV: SMTP unconfigured; welcome e-mail logado em vez de enviado."
            );
            return;
        };
        let origin = self.public_origin.trim_end_matches('/').to_owned();
        let db = self.db.clone();
        let smtp = smtp.clone();
        let to_owned = to.to_owned();
        tokio::spawn(async move {
            let mut ctx: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
            ctx.insert("site_url", origin.clone());
            ctx.insert("settings_url", format!("{origin}/configuracoes/"));
            let (subject, body) = dsoc_db::email_templates::render(&db, "welcome", &ctx)
                .await
                .unwrap_or_else(|| {
                    (
                        "Bem-vindo(a) à DemocraciaBR — sua conta está ativa".to_owned(),
                        format!(
                            "Olá,\n\nSua conta na DemocraciaBR está ativa.\n\n\
                             Comece por aqui: {origin}\n\n— DemocraciaBR"
                        ),
                    )
                });
            if let Err(err) = send_email(&smtp, &to_owned, &subject, &body).await {
                tracing::error!(error = ?err, "welcome e-mail send failed");
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
        // 0.32.1: plain text as fallback + branded HTML (html_wrap).
        .multipart(lettre::message::MultiPart::alternative_plain_html(
            body.to_owned(),
            dsoc_db::email_templates::html_wrap(body),
        ))?;
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

/// Mirrors `crate::service::map_register_sqlx`: a unique violation on email/cpf
/// at confirm becomes a Conflict with a specific message ("email_taken" / "cpf_taken").
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
        assert!(a
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
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
    fn slugify_name_folds_accents_and_separators() {
        assert_eq!(slugify_name("José da Silva"), "jose_da_silva");
        assert_eq!(slugify_name("  Maria  Antônia  "), "maria_antonia");
        assert_eq!(slugify_name("Ção Núñez"), "cao_nunez");
        // Symbols only → empty (the caller falls back to the automatic handle).
        assert_eq!(slugify_name("!!! ###"), "");
    }

    #[test]
    fn normalize_email_lowercases_and_validates() {
        assert_eq!(normalize_email("  Foo@Bar.com ").unwrap(), "foo@bar.com");
        assert!(normalize_email("no-at").is_err());
        assert!(normalize_email("a@b").is_err()); // sem ponto no domínio
    }
}

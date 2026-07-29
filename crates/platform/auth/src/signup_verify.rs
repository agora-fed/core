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
//!    link, achamos a pending pelo hash, e materializamos citizen, credential
//!    e sessão numa única transação (mark used, insert citizen, insert
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
/// Janela e teto do rate-limit por IP no `request` (P3.1). Configurável via
/// `AUTH_SIGNUP_RATE_MAX_PER_HOUR` (default 3). Sem override quando
/// `request_ip=None` — quem não vem com X-Forwarded-For não trava ninguém.
const DEFAULT_RATE_MAX_PER_HOUR: i64 = 3;

/// Papel do cadastro pendente. Determina qual materialização o `confirm`
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

/// Eleição-alvo do cadastro de candidato(a). Quando 2028 chegar, vira env/config.
const CANDIDATE_ELECTION_YEAR: i32 = 2026;

/// Metadados da candidatura auto-declarada (migration 0526). Validados no
/// request ([`CandidateMeta::validated`]), persistidos como jsonb na pending
/// e materializados no confirm (mandate + binding + candidacy).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CandidateMeta {
    /// Nome de urna (público).
    pub display_name: String,
    /// Cargo pretendido — mesmo eixo de `candidacy.office`.
    pub office: String,
    /// UF (obrigatória exceto presidente).
    pub uf: Option<String>,
    /// Município (obrigatório pra cargos municipais).
    pub municipio: Option<String>,
    /// Sigla do partido na filiação atual.
    pub party_sigla: String,
    /// Número de urna, se já definido pelo partido.
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
    /// [`Error::Validation`] com mensagem amigável pra cada campo inválido.
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

/// Resultado do `confirm`: sessão pronta (instância aberta) ou conta criada
/// mas travada em revisão manual (`GATEWAY_SIGNUP_REQUIRES_REVIEW`, 0514).
#[derive(Debug)]
pub enum ConfirmOutcome {
    Session(Box<IssuedSession>),
    PendingReview { email: String },
}

/// A instância exige aprovação manual de novos cadastros? Nome da env var
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
    identity_verifier: std::sync::Arc<dyn crate::identity_verify::IdentityVerifier>,
}

/// Dados de identidade auto-declarados no cadastro, confrontados com a base
/// autorizada (R-KYC #50). Todos opcionais por compat; o front novo envia
/// nome+nascimento+sexo (verificação) e, opcionalmente, o título de eleitor.
#[derive(Debug, Clone, Default)]
pub struct SignupIdentity {
    /// Nome completo informado.
    pub nome_completo: Option<String>,
    /// Data de nascimento `YYYY-MM-DD`.
    pub nascimento: Option<String>,
    /// Sexo `M`/`F`.
    pub sexo: Option<String>,
    /// Título de eleitor (opcional; sem ele = sem poder de voto).
    pub titulo_eleitor: Option<String>,
    /// UF de domicílio (sigla 2 letras). Obrigatória para cidadão (eixo territorial).
    pub uf: Option<String>,
    /// Município de domicílio (código IBGE). Obrigatório para cidadão; deve
    /// existir em `municipio_ibge` e pertencer à `uf`.
    pub municipio_ibge: Option<i32>,
    /// Nick escolhido pro fediverso (handle). Obrigatório pro cidadão (0664).
    pub handle: Option<String>,
}

/// Normaliza+valida um nick de fediverso: minúsculas, `[a-z0-9_]`, 3–30 chars.
/// Retorna o handle normalizado ou erro de validação. Usado no cadastro (0664).
pub fn normalize_handle(raw: &str) -> Result<String> {
    let h = raw.trim().trim_start_matches('@').to_lowercase();
    let ok = (3..=30).contains(&h.chars().count())
        && h.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
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

/// Slug ASCII de um nome pra derivar um handle (B4). Minúsculas, acentos comuns
/// do português dobrados pra ASCII, separadores (espaço/pontuação) viram `_` e o
/// resto é descartado. Ex.: "José da Silva" → "jose_da_silva".
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

/// Mapeia o sexo do formulário (`F`/`M`) para o vocabulário de `citizen.gender`.
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
    /// Constrói o serviço explicitamente, útil pra testes de integração que
    /// injetam `Db` + `Arc<dyn Clock>` diretos (sem passar por `AppState`
    /// nem env vars). Sempre em dev-mode (SMTP=None) — o e-mail é logado,
    /// não enviado.
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
            // Testes não têm serviço externo: Noop (skipped → cadastro segue).
            identity_verifier: std::sync::Arc::new(crate::identity_verify::NoopIdentityVerifier),
        }
    }

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
            // HTTP se CPF_VERIFY_URL estiver setada, senão Noop (degradação graciosa).
            identity_verifier: crate::identity_verify::from_env(),
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

    /// Passo 1c — candidato(a) SEM mandato (0526). Não há prova automática
    /// possível (a pessoa ainda não consta em registro oficial nenhum), então
    /// os metadados são validados e guardados como auto-declaração; o confirm
    /// materializa mandate `source='self'` + binding nível `email` + candidacy
    /// `listed=false`. A verificação (partido/TSE/admin) vem depois.
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
            None,
            SignupIdentity::default(),
            request_ip,
        )
        .await
    }

    // 8 args: o pipeline de signup é uma sequência linear de dados validados;
    // agrupar em struct só adicionaria uma camada sem callers extras (2 sites).
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
        // Rate-limit por IP ANTES de tudo — barato (uma query) e evita
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

        // R-KYC #50: quando nome+data+sexo vêm informados, confronta com a base
        // autorizada via o SaaS cpf-verify. REJEITA bloqueia o cadastro; as demais
        // faixas (ACEITA/REVISA/ESCALA) e Skipped (serviço ausente/fora — fail-open)
        // seguem. A revisão humana das faixas REVISA/ESCALA é tratada na fatia de
        // persistência (parte 2); aqui só barramos o negativo claro.
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

        // Domicílio obrigatório para cidadão (0651/0652/0653): UF + município IBGE,
        // e o município tem de existir e pertencer à UF. Político/candidato não
        // declaram domicílio aqui — o território vem do mandato/candidatura.
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

        // Dados pessoais do cidadão. OBRIGATÓRIO no cadastro: nome (com sobrenome)
        // e nascimento (este último alimenta a verificação R-KYC junto do CPF).
        // OPCIONAL (B4 — onboarding enxuto): sexo (ProfileGate coleta depois) e o
        // nick do fediverso (derivado do nome; editável em Configurações).
        // Político/candidato herdam nome/território do mandato/candidatura → None.
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
            // B4: sexo OPCIONAL. Quando ausente fica None; o ProfileGate coleta
            // depois (profile-status já lista "sexo" como faltante).
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
            // B4: nick não é mais pedido no cadastro. Se o cidadão informou um
            // (fluxo avançado/futuro), respeitamos e validamos; senão derivamos do
            // nome. handle=None faz o confirm() cair no automático `cidadao-<id8>`.
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

    /// Deriva um handle único a partir do nome (B4 — nick não é mais pedido no
    /// cadastro). Faz o slug ASCII do nome, garante 1º caractere-letra e 3–30
    /// chars, e resolve colisão com sufixo numérico. Retorna `None` quando não há
    /// base utilizável (nome só com símbolos) ou todas as variantes já estão
    /// tomadas — nesse caso o `confirm()` gera o automático `cidadao-<id8>`.
    async fn derive_unique_handle(&self, org: Uuid, name: &str) -> Result<Option<String>> {
        let mut base = slugify_name(name);
        // 1º caractere precisa ser letra (regra do handle).
        if !base.chars().next().is_some_and(|c| c.is_ascii_lowercase()) {
            base.insert(0, 'c');
        }
        // Deixa folga pra um sufixo numérico dentro do teto de 30 chars. base é
        // ASCII puro (slugify), então truncar por bytes é seguro.
        base.truncate(26);
        while base.ends_with('_') {
            base.pop();
        }
        if base.len() < 3 {
            return Ok(None);
        }
        // Tenta o base puro e depois base2..=base9. O confirm() re-checa a
        // disponibilidade e cai no automático se perder a corrida request→confirm.
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

    /// Reenvia o link de verificação pra um e-mail que tem pending viva. Se
    /// não houver pending live, silenciosamente não faz nada — resposta 200
    /// no wire independe (enumeration-safe, mesmo padrão do password_reset).
    ///
    /// Reaproveita password_hash+cpf+role+mandate_id da pending existente
    /// (o usuário não digita nada de novo). Gera token novo, invalida a
    /// pending anterior, insere nova, dispara e-mail.
    ///
    /// # Errors
    /// [`Error::Storage`] em falha dura de persistência.
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

    /// Cleanup worker (P3.3): remove pendings expiradas há mais de
    /// `cutoff_days` dias. Retorna quantas foram apagadas — útil pra métricas.
    ///
    /// # Errors
    /// [`Error::Storage`] em falha de DELETE.
    pub async fn cleanup_expired(&self, cutoff_days: i64) -> Result<u64> {
        let cutoff = self.clock.now() - chrono::Duration::days(cutoff_days.max(1));
        queries::pending_signup_cleanup_expired(&self.db, cutoff)
            .await
            .map_err(map_sqlx)
    }

    /// Cleanup do `auth_login_attempt` (P5.1). Não faz parte do serviço de
    /// signup em si, mas rodamos no mesmo worker pra economizar loops. Usa
    /// o mesmo cutoff_days por consistência operacional.
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

    /// Passo 2 — redime o token e materializa a conta. Em instância aberta
    /// retorna a sessão pronta pra o handler HTTP setar o cookie; quando
    /// `GATEWAY_SIGNUP_REQUIRES_REVIEW` está ligada (migration 0514), a conta
    /// nasce com `pending_review = true`, NENHUMA sessão é emitida e o
    /// outcome diz ao front que falta aprovação de admin (/admin/revisoes).
    ///
    /// # Errors
    /// [`Error::Unauthorized`] pra token inválido/expirado/já usado
    /// (deliberadamente opaco);
    /// [`Error::Conflict`] se e-mail ou CPF já foram levados por outro cadastro
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
            // Candidato auto-declarado NÃO ganha 'directory' — não há registro
            // oficial pra conferir. Sobe via atestação/TSE/admin depois.
            PendingRole::Cidadao | PendingRole::Candidato => match cpf_status {
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
        // Dados pessoais do cadastro (0664): nome → display_name + legal_name (o nome
        // civil informado é o mesmo que aparece), sexo → gender, nascimento → birth_date.
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

        // Handle do fediverso: usa o NICK escolhido no cadastro (0664) se ainda estiver
        // livre; senão cai no automático `cidadao-<id8>` (único por construção). Político/
        // candidato (sem nick no cadastro) também caem no automático — trocam depois.
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

        if role == PendingRole::Candidato {
            // Mesma transparência não-opt-out do político: candidatura é pública.
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
            // Defensivo: re-valida o que o request gravou (o CHECK do banco só
            // garante presença do jsonb, não o shape).
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
            // Binding nível 'email' — o gate is_politico destrava (painel +
            // /me/campanha), mas o selo público segue "autodeclarada".
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
            // Candidatura fora do comparador (listed=false) até verificação.
            // Sem eleição correspondente cadastrada, segue sem candidacy — o
            // mandato/binding bastam pras ferramentas.
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
        // Onboarding federado pós-commit, best-effort (falha aqui NUNCA derruba o
        // cadastro): chaves do ator (perfil resolvível no fediverso desde o 1º
        // segundo) + follow automático do perfil oficial @socrates (comunicados).
        self.federated_onboarding(citizen, now).await;
        // Boas-vindas só depois do commit — conta existe de fato.
        self.deliver_welcome(&row.email);
        Ok(ConfirmOutcome::Session(Box::new(session)))
    }

    /// Chaves do ator + auto-follow do @socrates. Best-effort com log.
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
        // Template editável pelo admin (0.32.0); fallback hardcoded se a
        // linha sumiu do DB — o e-mail de confirmação nunca deixa de sair.
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
        // Non-blocking: a request retorna mesmo se o relay travar. Falha de
        // envio só é auditada — mesma escolha do password_reset.
        tokio::spawn(async move {
            if let Err(err) = send_email(&smtp, &to_owned, &subject, &body).await {
                tracing::error!(error = ?err, "signup-verify e-mail send failed");
            }
        });
    }

    /// Boas-vindas pós-ativação (0.32.0): dispara depois que `confirm()`
    /// materializa a conta. Best-effort e não-bloqueante — falha de SMTP
    /// nunca atrapalha a sessão recém-criada.
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
        // 0.32.1: texto puro como fallback + HTML com a marca (html_wrap).
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
        // Só símbolos → vazio (caller cai no handle automático).
        assert_eq!(slugify_name("!!! ###"), "");
    }

    #[test]
    fn normalize_email_lowercases_and_validates() {
        assert_eq!(normalize_email("  Foo@Bar.com ").unwrap(), "foo@bar.com");
        assert!(normalize_email("no-at").is_err());
        assert!(normalize_email("a@b").is_err()); // sem ponto no domínio
    }
}

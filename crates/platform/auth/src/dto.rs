//! The crate's own request/response DTOs plus their `utoipa` schema fragment (ADR-0004:
//! `api-contract` holds only the envelope/error/pagination; each crate owns its shapes and the
//! gateway composes `/openapi.json`). Domain types are mapped to these at the HTTP boundary.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::domain;
use crate::service::{Identity, IssuedSession};

/// `POST /auth/session` body: exchange a validated OIDC token for a session.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CreateSessionRequest {
    /// Organization/tenant the session belongs to.
    pub org_id: Uuid,
    /// The OIDC token to validate (typically the Zitadel ID/access token).
    pub token: String,
}

/// Public view of an issued session. The OIDC subject is intentionally NOT exposed.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SessionDto {
    /// Opaque session id.
    pub id: Uuid,
    /// The authenticated citizen.
    pub citizen_id: Uuid,
    /// When the session was issued.
    pub issued_at: DateTime<Utc>,
    /// When the session expires.
    pub expires_at: DateTime<Utc>,
    /// Stable public ActivityPub-readiness handle (ADR-0005).
    pub public_handle: String,
}

impl From<IssuedSession> for SessionDto {
    fn from(session: IssuedSession) -> Self {
        Self {
            id: session.id,
            citizen_id: session.citizen.as_uuid(),
            issued_at: session.issued_at,
            expires_at: session.expires_at,
            public_handle: session.public_handle,
        }
    }
}

/// `GET /auth/me` response: the identity behind the presented token.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MeDto {
    /// The citizen.
    pub citizen_id: Uuid,
    /// Verification level (`anonymous` | `email` | `directory` | `strong`).
    pub verification_level: String,
    /// Stable public ActivityPub-readiness handle (ADR-0005).
    pub public_handle: String,
}

impl From<Identity> for MeDto {
    fn from(identity: Identity) -> Self {
        Self {
            citizen_id: identity.citizen.as_uuid(),
            verification_level: domain::level_as_str(identity.level).to_owned(),
            public_handle: identity.public_handle,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use dsoc_core::ids::CitizenId;
    use dsoc_core::VerificationLevel;

    #[test]
    fn me_dto_maps_level_to_snake_case_string() {
        let identity = Identity {
            citizen: CitizenId::new(),
            oidc_subject: "sub".into(),
            level: VerificationLevel::Directory,
            public_handle: "u-abc".into(),
        };
        let dto = MeDto::from(identity);
        assert_eq!(dto.verification_level, "directory");
        assert_eq!(dto.public_handle, "u-abc");
    }

    #[test]
    fn session_dto_hides_oidc_subject() {
        let citizen = CitizenId::new();
        let session = IssuedSession {
            id: Uuid::now_v7(),
            citizen,
            oidc_subject: "secret-subject".into(),
            issued_at: Utc::now(),
            expires_at: Utc::now(),
            public_handle: "u-xyz".into(),
        };
        let dto = SessionDto::from(session);
        let json = serde_json::to_string(&dto).unwrap();
        assert!(!json.contains("secret-subject"));
        assert_eq!(dto.citizen_id, citizen.as_uuid());
    }
}

/// Registration: e-mail + senha + CPF (auth verified by CPF, not an external IdP).
#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct RegisterRequest {
    /// Organization/tenant the citizen registers under.
    pub org_id: uuid::Uuid,
    /// Contact e-mail (unique per org).
    pub email: String,
    /// Password (>= 8 chars; stored Argon2id-hashed, never plaintext).
    pub password: String,
    /// Brazilian CPF (any punctuation; validated by check digits).
    pub cpf: String,
    /// Full name — matched against the authorized base (R-KYC). Optional for
    /// compat: when present with birth date + sex, the signup is verified and blocks
    /// em REJEITA. O front novo sempre envia.
    #[serde(default)]
    pub nome_completo: Option<String>,
    /// Date of birth `YYYY-MM-DD`, for verification.
    #[serde(default)]
    pub nascimento: Option<String>,
    /// Sex `M`/`F`, for verification. OPTIONAL at signup (B4): when absent,
    /// ProfileGate collects it later. It does not disable R-KYC (which runs on name+birth).
    #[serde(default)]
    pub sexo: Option<String>,
    /// Electoral registry (OPTIONAL). Without it, the citizen has no valid voting
    /// power in the system (surfaced in the UI).
    #[serde(default)]
    pub titulo_eleitor: Option<String>,
    /// Residence UF (2-letter code). MANDATORY at citizen signup —
    /// it is the territorial axis (state scope). Validated against `municipio_ibge`.
    #[serde(default)]
    pub uf: Option<String>,
    /// Residence municipality (7-digit IBGE code). MANDATORY at citizen
    /// signup — municipal scope. Must exist and belong to the given `uf`.
    #[serde(default)]
    pub municipio_ibge: Option<i32>,
    /// Fediverse nick (chosen handle). OPTIONAL at signup (B4): when absent,
    /// the backend derives it from the name (editable later in Settings). When
    /// given: 3–30 chars, `[a-z0-9_]`, starts with a letter; unique per org.
    #[serde(default)]
    pub handle: Option<String>,
}

/// Registration for a sitting/candidate parliamentarian: the standard sign-up fields plus
/// a chosen `mandate_id`. The mandate's `public_email` MUST equal `email` — that is the
/// only credential we can automatically verify without an external OOB channel; anything
/// weaker would let a random citizen self-declare as any politician. When the check passes
/// the flow (a) creates the citizen and credential, (b) sets `is_public=true` (transparency
/// is mandatory for mandates), and (c) writes an `mandate_identity_binding` at level
/// `directory`, atomically. Runs against `POST /auth/register/politician` (F1.3/F1.4).
#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct RegisterPoliticianRequest {
    /// Organization/tenant the mandate lives in.
    pub org_id: uuid::Uuid,
    /// Contact e-mail — MUST match the mandate's `public_email` (case-insensitive).
    pub email: String,
    /// Password (>= 8 chars; stored Argon2id-hashed).
    pub password: String,
    /// Brazilian CPF (any punctuation; validated by check digits).
    pub cpf: String,
    /// Mandate the citizen operates.
    pub mandate_id: uuid::Uuid,
}

/// Cadastro de candidato(a) SEM mandato (auto-declarado, migration 0526).
/// Confirm materializes a mandate with `source='self'` + an `email`-level binding +
/// candidacy `listed=false`. Runs against `POST /auth/register/candidate`.
#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct RegisterCandidateRequest {
    /// Organization/tenant.
    pub org_id: uuid::Uuid,
    /// Contact e-mail (verificado por link, como no register comum).
    pub email: String,
    /// Password (>= 8 chars; stored Argon2id-hashed).
    pub password: String,
    /// Brazilian CPF (any punctuation; validated by check digits).
    pub cpf: String,
    /// Ballot name (public).
    pub display_name: String,
    /// Cargo pretendido: presidente | governador | senador | deputado_federal
    /// | deputado_estadual | prefeito | vice_prefeito | vereador.
    pub office: String,
    /// UF (mandatory except for president).
    pub uf: Option<String>,
    /// Municipality (mandatory for municipal offices).
    pub municipio: Option<String>,
    /// Party sigla of the current affiliation.
    pub party_sigla: String,
    /// Ballot number, when already assigned.
    pub number: Option<String>,
}

/// Login with e-mail + senha.
#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct LoginRequest {
    /// Organization/tenant.
    pub org_id: uuid::Uuid,
    /// Registered e-mail.
    pub email: String,
    /// Password.
    pub password: String,
}

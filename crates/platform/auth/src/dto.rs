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

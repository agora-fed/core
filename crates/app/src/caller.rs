//! The authenticated caller (ADR-0007). Handlers MUST take the acting citizen's identity from this
//! extractor — never from the request body — closing the recurring "trust citizen_id from the body"
//! authorization bypass. The gateway's auth middleware validates the Zitadel OIDC token and sets the
//! caller identity; until that middleware lands, the caller is read from the trusted, gateway-set
//! `x-dsoc-citizen-id` / `x-dsoc-org-id` headers (which the public ingress strips from clients).

use axum::extract::FromRequestParts;
use axum::http::{request::Parts, StatusCode};
use dsoc_core::ids::{CitizenId, OrgId};
use uuid::Uuid;

/// The verified identity of the request's caller.
#[derive(Debug, Clone, Copy)]
pub struct CallerId {
    /// The authenticated citizen.
    pub citizen: CitizenId,
    /// The organization/tenant context.
    pub org: OrgId,
}

fn header_uuid(parts: &Parts, name: &str) -> Option<Uuid> {
    parts.headers.get(name)?.to_str().ok()?.parse().ok()
}

impl<S: Send + Sync> FromRequestParts<S> for CallerId {
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let citizen = header_uuid(parts, "x-dsoc-citizen-id")
            .map(CitizenId::from_uuid)
            .ok_or((StatusCode::UNAUTHORIZED, "missing authenticated caller"))?;
        let org = header_uuid(parts, "x-dsoc-org-id")
            .map(OrgId::from_uuid)
            .ok_or((StatusCode::UNAUTHORIZED, "missing org context"))?;
        Ok(CallerId { citizen, org })
    }
}

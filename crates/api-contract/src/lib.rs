//! # dsoc-api-contract — Tier-0 public API contract
//!
//! The shared, frozen surface that **web and mobile clients consume as peers**
//! (PLAN.md principle 9). Tier-3 clients depend on this crate (and its OpenAPI doc)
//! only — never on server implementations — so they can build against mocks while the
//! backend is still under construction (PLAN.md section 6.3).

pub mod dto;
pub mod envelope;

pub use dto::{
    MandateDto, MyMandateDto, ProfileDto, ProfileUpdateDto, ProposalDto, ScorecardDto,
    SessionInfoDto, SlaStatus,
};
pub use envelope::{ApiError, ApiResponse, PageMeta};

use utoipa::OpenApi;

/// The generated OpenAPI 3.1 document for the public API surface.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "democracia-social API",
        version = "0.1.0",
        description = "Sovereign participatory democracy platform — public API (codename PINDORAMA)."
    ),
    components(schemas(
        dto::ProposalDto,
        dto::MandateDto,
        dto::MyMandateDto,
        dto::ProfileDto,
        dto::ProfileUpdateDto,
        dto::ScorecardDto,
        dto::SessionInfoDto,
        dto::SlaStatus,
        envelope::ApiError,
        envelope::PageMeta,
    ))
)]
#[derive(Debug)]
pub struct ApiDoc;

/// Serialize the OpenAPI document as pretty JSON (used by CI to publish the contract).
///
/// # Panics
/// Never in practice; the document is statically derived.
#[must_use]
pub fn openapi_json() -> String {
    ApiDoc::openapi().to_pretty_json().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_document_is_generated_and_titled() {
        let json = openapi_json();
        assert!(json.contains("democracia-social API"));
        assert!(json.contains("ProposalDto"));
    }
}

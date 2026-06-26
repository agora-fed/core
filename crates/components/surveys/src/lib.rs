//! # dsoc-surveys
//!
//! Tier 2 crate. Surveys: typed questionnaires with one response per citizen.
//!
//! ## Contract
//! - **Emits:** (none) — `surveys.*` is not part of the frozen
//!   [`dsoc_core::events::Event`] catalog and nothing consumes it, so this crate keeps its
//!   state private and exposes it only through its routes (the same posture as `dsoc-admin`).
//! - **Consumes:** (none)
//! - **Owns tables:** `survey`, `survey_question`, `survey_response`
//!
//! This crate talks to the rest of the system ONLY through `dsoc-core` traits, `dsoc-app`
//! state, and the gateway. It never reaches into another crate's internals and never depends
//! on a peer `dsoc-*` crate (see `DO NOT` in PLAN.md and CONTRIBUTING.md).

#![forbid(unsafe_code)]

pub mod domain;
pub mod dto;
pub mod http;
pub mod queries;
pub mod service;

pub use domain::{NewQuestion, NewResponse, NewSurvey, QuestionKind};
pub use http::routes;
pub use queries::{QuestionRow, ResponseRow, SurveyRow};
pub use service::SurveyService;

use dsoc_core::ids::MandateId;
use dsoc_core::traits::Component;

/// Compile-time marker proving the crate name is wired into the workspace.
pub const CRATE_NAME: &str = "dsoc-surveys";

/// The stable machine name of this participation component.
pub const COMPONENT_KIND: &str = "surveys";

/// The `surveys` participation component (a typed questionnaire). Implements
/// [`dsoc_core::traits::Component`] so a space can mount it. A survey is not directed at a
/// mandate (it does not drive the consequence loop), so [`Component::directed_mandate`] is
/// always `None`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SurveysComponent;

impl Component for SurveysComponent {
    fn kind(&self) -> &'static str {
        COMPONENT_KIND
    }

    fn directed_mandate(&self) -> Option<MandateId> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_stable() {
        // Guards against accidental rename that would break wiring.
        assert_eq!(CRATE_NAME, "dsoc-surveys");
    }

    #[test]
    fn component_kind_is_surveys_and_undirected() {
        let component = SurveysComponent;
        assert_eq!(component.kind(), "surveys");
        assert!(
            component.directed_mandate().is_none(),
            "a survey does not direct the consequence loop"
        );
    }
}

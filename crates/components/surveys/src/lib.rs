//! # dsoc-surveys
//!
//! Tier 2 crate. Surveys: structured questionnaires with typed answers and tallies.
//!
//! ## Contract
//! - **Emits:** surveys.published, surveys.response.received
//! - **Consumes:** (none)
//! - **Owns tables:** survey, survey_question, survey_response
//!
//! This crate talks to the rest of the system ONLY through `dsoc-core` traits,
//! the event bus (`dsoc-events`), and the gateway. It never reaches into another
//! crate's internals (see `DO NOT` in PLAN.md and CONTRIBUTING.md).

#![forbid(unsafe_code)]

/// Compile-time marker proving the crate name is wired into the workspace.
pub const CRATE_NAME: &str = "dsoc-surveys";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_stable() {
        // Guards against accidental rename that would break event routing.
        assert_eq!(CRATE_NAME, "dsoc-surveys");
    }
}

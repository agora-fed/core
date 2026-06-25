//! # dsoc-moderation
//!
//! Tier 1 crate. Auditable moderation: deterministic rules + statistical anomaly detection + optional local model. No opaque third-party classifiers (principle 11).
//!
//! ## Contract
//! - **Emits:** moderation.flagged, moderation.cleared
//! - **Consumes:** proposals.created, comments.created
//! - **Owns tables:** moderation_rule, moderation_decision, moderation_appeal
//!
//! This crate talks to the rest of the system ONLY through `dsoc-core` traits,
//! the event bus (`dsoc-events`), and the gateway. It never reaches into another
//! crate's internals (see `DO NOT` in PLAN.md and CONTRIBUTING.md).

#![forbid(unsafe_code)]

pub mod domain;
pub mod events;
pub mod http;
pub mod queries;
pub mod service;

pub use events::{emit_decision, handle_event};
pub use http::routes;
pub use service::{ModerationService, ModerationTarget};

/// Compile-time marker proving the crate name is wired into the workspace.
pub const CRATE_NAME: &str = "dsoc-moderation";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_stable() {
        // Guards against accidental rename that would break event routing.
        assert_eq!(CRATE_NAME, "dsoc-moderation");
    }
}

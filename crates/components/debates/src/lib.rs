//! # dsoc-debates
//!
//! Tier 2 crate. Structured debates: framed pro/con deliberation spaces.
//!
//! ## Contract
//! - **Emits:** (none) — `debates.*` is not part of the frozen
//!   [`dsoc_core::events::Event`] catalog and nothing consumes it, so this crate keeps its
//!   state private and exposes it only through its routes (the same posture as `dsoc-admin`).
//! - **Consumes:** (none)
//! - **Owns tables:** `debate`, `debate_contribution`
//!
//! This crate talks to the rest of the system ONLY through `dsoc-core` traits, `dsoc-app`
//! state, and the gateway. It never reaches into another crate's internals and never depends
//! on a peer `dsoc-*` crate (see `DO NOT` in PLAN.md and CONTRIBUTING.md).

#![forbid(unsafe_code)]

pub mod domain;
pub mod http;
pub mod queries;
pub mod service;

pub use domain::{NewContribution, NewDebate, Stance};
pub use http::routes;
pub use queries::{ContributionRow, DebateRow};
pub use service::DebateService;

use dsoc_core::ids::MandateId;
use dsoc_core::traits::Component;

/// Compile-time marker proving the crate name is wired into the workspace.
pub const CRATE_NAME: &str = "dsoc-debates";

/// The stable machine name of this participation component.
pub const COMPONENT_KIND: &str = "debates";

/// The `debates` participation component (a framed pro/con deliberation space). Implements
/// [`dsoc_core::traits::Component`] so a space can mount it. A debate is not directed at a
/// mandate (it does not drive the consequence loop), so [`Component::directed_mandate`] is
/// always `None`.
#[derive(Debug, Clone, Copy, Default)]
pub struct DebatesComponent;

impl Component for DebatesComponent {
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
        assert_eq!(CRATE_NAME, "dsoc-debates");
    }

    #[test]
    fn component_kind_is_debates_and_undirected() {
        let component = DebatesComponent;
        assert_eq!(component.kind(), "debates");
        assert!(
            component.directed_mandate().is_none(),
            "a debate does not direct the consequence loop"
        );
    }
}

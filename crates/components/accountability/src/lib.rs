//! # dsoc-accountability
//!
//! Tier 2 crate. Accountability: the public record of RESULTS and PROGRESS of commitments made
//! inside spaces.
//!
//! ## Contract
//! - **Emits:** (none) — `accountability.*` is not part of the frozen
//!   [`dsoc_core::events::Event`] catalog and nothing consumes it yet, so this crate keeps its
//!   state private and exposes it only through its routes (the same posture as `dsoc-admin`,
//!   per YAGNI / ADR-0004; deferred until a consumer needs it).
//! - **Consumes:** (none)
//! - **Owns tables:** `accountability_result`, `accountability_status_update`
//!
//! This crate talks to the rest of the system ONLY through `dsoc-core` traits, `dsoc-app` state,
//! and the gateway. It never reaches into another crate's internals and never depends on a peer
//! `dsoc-*` crate (see `DO NOT` in PLAN.md and CONTRIBUTING.md).

#![forbid(unsafe_code)]

pub mod domain;
pub mod http;
pub mod queries;
pub mod service;

pub use http::routes;
pub use service::AccountabilityService;

use dsoc_core::ids::MandateId;
use dsoc_core::traits::Component;

/// Compile-time marker proving the crate name is wired into the workspace.
pub const CRATE_NAME: &str = "dsoc-accountability";

/// The stable machine name of this participation component.
pub const COMPONENT_KIND: &str = "accountability";

/// The `accountability` participation component (results/progress tracking of commitments).
/// Implements [`dsoc_core::traits::Component`] so a space can mount it. Accountability tracks
/// commitments at the org level rather than directing a single mandate, so
/// [`Component::directed_mandate`] is always `None`.
#[derive(Debug, Clone, Copy, Default)]
pub struct AccountabilityComponent;

impl Component for AccountabilityComponent {
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
        assert_eq!(CRATE_NAME, "dsoc-accountability");
    }

    #[test]
    fn component_kind_is_accountability_and_undirected() {
        let component = AccountabilityComponent;
        assert_eq!(component.kind(), "accountability");
        assert!(
            component.directed_mandate().is_none(),
            "accountability tracks org-level commitments, not a single mandate"
        );
    }
}

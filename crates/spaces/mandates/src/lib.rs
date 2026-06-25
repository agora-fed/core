//! # dsoc-mandates
//!
//! Tier 2 crate. NEW. Mandate & candidate registry: ingests public official/candidate directories (Camara, Senado, prefeituras, TSE), binds each to a public email, drives mandatory onboarding (Decidim failure #6).
//!
//! ## Contract
//! - **Emits:** mandates.official.invited, mandates.official.onboarded, mandates.identity.verified
//! - **Consumes:** auth.verification.upgraded
//! - **Owns tables:** mandate, mandate_office, mandate_invitation, mandate_identity_binding
//!
//! This crate talks to the rest of the system ONLY through `dsoc-core` traits,
//! the event bus (`dsoc-events`), and the gateway. It never reaches into another
//! crate's internals (see `DO NOT` in PLAN.md and CONTRIBUTING.md).

#![forbid(unsafe_code)]

/// Compile-time marker proving the crate name is wired into the workspace.
pub const CRATE_NAME: &str = "dsoc-mandates";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_stable() {
        // Guards against accidental rename that would break event routing.
        assert_eq!(CRATE_NAME, "dsoc-mandates");
    }
}

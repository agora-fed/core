//! # dsoc-auth
//!
//! Tier 1 crate. Sovereign identity & verification: Zitadel/OIDC token validation, session issuance, and answering "is this person who they claim?" at graded verification levels.
//!
//! ## Contract
//! - **Emits:** auth.session.created, auth.verification.upgraded
//! - **Consumes:** mandates.official.invited
//! - **Owns tables:** auth_session, auth_verification_level
//!
//! This crate talks to the rest of the system ONLY through `dsoc-core` traits,
//! the event bus (`dsoc-events`), and the gateway. It never reaches into another
//! crate's internals (see `DO NOT` in PLAN.md and CONTRIBUTING.md).

#![forbid(unsafe_code)]

/// Compile-time marker proving the crate name is wired into the workspace.
pub const CRATE_NAME: &str = "dsoc-auth";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_stable() {
        // Guards against accidental rename that would break event routing.
        assert_eq!(CRATE_NAME, "dsoc-auth");
    }
}

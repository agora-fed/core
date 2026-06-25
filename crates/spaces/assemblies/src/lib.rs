//! # dsoc-assemblies
//!
//! Tier 2 crate. Assemblies: permanent participatory bodies with membership and recurring decisions.
//!
//! ## Contract
//! - **Emits:** assemblies.created
//! - **Consumes:** (none)
//! - **Owns tables:** assembly, assembly_member
//!
//! This crate talks to the rest of the system ONLY through `dsoc-core` traits,
//! the event bus (`dsoc-events`), and the gateway. It never reaches into another
//! crate's internals (see `DO NOT` in PLAN.md and CONTRIBUTING.md).

#![forbid(unsafe_code)]

/// Compile-time marker proving the crate name is wired into the workspace.
pub const CRATE_NAME: &str = "dsoc-assemblies";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_stable() {
        // Guards against accidental rename that would break event routing.
        assert_eq!(CRATE_NAME, "dsoc-assemblies");
    }
}

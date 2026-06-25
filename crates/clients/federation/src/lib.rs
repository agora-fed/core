//! # dsoc-federation
//!
//! Tier 3 crate. Federation hub & SDK (Phase 3): lets a municipality run a local instance that federates its signal into the central platform. Depends only on the frozen api-contract.
//!
//! ## Contract
//! - **Emits:** federation.instance.registered, federation.signal.synced
//! - **Consumes:** (none)
//! - **Owns tables:** federation_instance, federation_sync_cursor
//!
//! This crate talks to the rest of the system ONLY through `dsoc-core` traits,
//! the event bus (`dsoc-events`), and the gateway. It never reaches into another
//! crate's internals (see `DO NOT` in PLAN.md and CONTRIBUTING.md).

#![forbid(unsafe_code)]

/// Compile-time marker proving the crate name is wired into the workspace.
pub const CRATE_NAME: &str = "dsoc-federation";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_stable() {
        // Guards against accidental rename that would break event routing.
        assert_eq!(CRATE_NAME, "dsoc-federation");
    }
}

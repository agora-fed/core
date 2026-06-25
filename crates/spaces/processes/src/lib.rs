//! # dsoc-processes
//!
//! Tier 2 crate. Participatory processes: time-bound civic spaces grouping components toward a goal.
//!
//! ## Contract
//! - **Emits:** processes.created, processes.phase.advanced
//! - **Consumes:** (none)
//! - **Owns tables:** process, process_phase
//!
//! This crate talks to the rest of the system ONLY through `dsoc-core` traits,
//! the event bus (`dsoc-events`), and the gateway. It never reaches into another
//! crate's internals (see `DO NOT` in PLAN.md and CONTRIBUTING.md).

#![forbid(unsafe_code)]

/// Compile-time marker proving the crate name is wired into the workspace.
pub const CRATE_NAME: &str = "dsoc-processes";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_stable() {
        // Guards against accidental rename that would break event routing.
        assert_eq!(CRATE_NAME, "dsoc-processes");
    }
}

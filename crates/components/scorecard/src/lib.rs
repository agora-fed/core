//! # dsoc-scorecard
//!
//! Tier 2 crate. NEW. Persistent, public, per-politician record: promises vs delivery, answered vs ignored, response latency. The accountability artifact Decidim never produced (failure #7).
//!
//! ## Contract
//! - **Emits:** scorecard.updated
//! - **Consumes:** consequence.sla.expired, consequence.official.responded, mandates.official.onboarded
//! - **Owns tables:** scorecard, scorecard_entry, scorecard_promise
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
pub mod tier;

pub use events::{handle_event, scorecard_updated};
pub use tier::{
    average_rate, better_than_pct, current_answer_streak, responds_in_days, response_rate_pct,
    responsiveness_tier, top_pct, ResponsivenessTier,
};
pub use http::routes;
pub use service::ScorecardService;

/// Compile-time marker proving the crate name is wired into the workspace.
pub const CRATE_NAME: &str = "dsoc-scorecard";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_stable() {
        // Guards against accidental rename that would break event routing.
        assert_eq!(CRATE_NAME, "dsoc-scorecard");
    }
}

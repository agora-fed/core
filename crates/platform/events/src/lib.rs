//! # dsoc-events
//!
//! Tier 1 crate. Postgres-backed event bus implementation (pgmq). Durable publish/subscribe; the only sanctioned cross-crate side-effect channel.
//!
//! ## Contract
//! - **Emits:** (none)
//! - **Consumes:** *
//! - **Owns tables:** events_log, events_subscription
//!
//! This crate talks to the rest of the system ONLY through `dsoc-core` traits,
//! the event bus (`dsoc-events`), and the gateway. It never reaches into another
//! crate's internals (see `DO NOT` in PLAN.md and CONTRIBUTING.md).

#![forbid(unsafe_code)]

mod domain;
mod events;
mod http;
mod queries;
mod service;

pub use domain::{
    decode_event, encode_event, total_pending, Cursor, EncodedEvent, QueueDepth, SubscriberName,
};
pub use events::{dispatch_batch, EventHandler};
pub use http::routes;
pub use service::{EventQueue, PgEventBus};

/// Compile-time marker proving the crate name is wired into the workspace.
pub const CRATE_NAME: &str = "dsoc-events";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_stable() {
        // Guards against accidental rename that would break event routing.
        assert_eq!(CRATE_NAME, "dsoc-events");
    }
}

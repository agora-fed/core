//! # dsoc-notify
//!
//! Tier 1 crate. Multi-channel fan-out: push (mobile), email (SMTP), and WhatsApp/Chatwoot. Owns delivery receipts and retry/backoff.
//!
//! ## Contract
//! - **Emits:** notify.dispatched, notify.delivery.failed
//! - **Consumes:** proposals.threshold.crossed, consequence.sla.started, consequence.sla.expired,
//!   proposals.published
//! - **Owns tables:** notify_outbox, notify_delivery_receipt, notify_device_token
//!
//! This crate talks to the rest of the system ONLY through `dsoc-core` traits,
//! the event bus (`dsoc-events`), and the gateway. It never reaches into another
//! crate's internals (see `DO NOT` in PLAN.md and CONTRIBUTING.md).
//!
//! ## Module map
//! - [`domain`] — pure value objects, validation, retry state machine (unit-tested).
//! - [`queries`] — compile-time-checked `sqlx` statements (no ORM, no `SELECT *`).
//! - [`service`] — [`service::NotifyService`]: enqueue → dispatch → receipt + error mapping.
//! - [`events`] — emit `notify.*` via the injected `EventBus`.
//! - [`sla_subscriber`] — the idempotent consume seam for the SLA-lifecycle events (looks up the
//!   mandate's operator and delegates to [`service::NotifyService::enqueue_for_event`]).
//! - [`http`] — Axum [`routes`] mounted by the gateway (binds no socket).

#![forbid(unsafe_code)]

pub mod domain;
pub mod events;
pub mod http;
pub mod queries;
pub mod service;
pub mod sla_subscriber;

pub use http::routes;
pub use service::{
    Channel, ChannelError, DeliveryReport, NoopChannel, NotifyService, OutboundMessage,
};
pub use sla_subscriber::handle_event as handle_sla_event;

/// Compile-time marker proving the crate name is wired into the workspace.
pub const CRATE_NAME: &str = "dsoc-notify";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_stable() {
        // Guards against accidental rename that would break event routing.
        assert_eq!(CRATE_NAME, "dsoc-notify");
    }
}

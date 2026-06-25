//! # dsoc-notify
//!
//! Tier 1 crate. Multi-channel fan-out: push (mobile), email (SMTP), and WhatsApp/Chatwoot. Owns delivery receipts and retry/backoff.
//!
//! ## Contract
//! - **Emits:** notify.dispatched, notify.delivery.failed
//! - **Consumes:** consequence.sla.started, consequence.sla.expired, proposals.published
//! - **Owns tables:** notify_outbox, notify_delivery_receipt, notify_device_token
//!
//! This crate talks to the rest of the system ONLY through `dsoc-core` traits,
//! the event bus (`dsoc-events`), and the gateway. It never reaches into another
//! crate's internals (see `DO NOT` in PLAN.md and CONTRIBUTING.md).

#![forbid(unsafe_code)]

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

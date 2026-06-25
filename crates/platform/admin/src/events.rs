//! Cross-crate event surface for `dsoc-admin` — intentionally empty.
//!
//! The frozen event catalog (`dsoc_core::events::Event`) has **no `admin.*` variants**,
//! and adding one is a Tier-0 change requiring an ADR (PLAN.md section 5.3). Administration
//! is internal state management — creating an org extension, binding a role, toggling a
//! feature flag — none of which the rest of the system subscribes to today.
//!
//! Therefore admin **persists state and exposes routes but emits no cross-crate events**.
//! The publish port is still injected and held by [`crate::service::AdminService`] (exposed
//! via `AdminService::event_bus`) so that, when a future ADR adds the relevant variants, the
//! emission can be wired here without touching the service's construction or the gateway.
//!
//! Admin likewise consumes no events: it owns no projection that another crate's events feed.
//!
//! This module is deliberately item-free; it documents a binding contract decision so the
//! absence of events is explicit and auditable rather than an oversight.

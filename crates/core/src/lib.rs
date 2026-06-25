//! # dsoc-core — Tier-0 frozen contract
//!
//! The single, sequentially-owned crate every other crate depends on **read-only**
//! (PLAN.md sections 5.3, 6.1). It defines the domain vocabulary shared across the
//! whole workspace:
//!
//! - [`ids`] — strongly-typed identifier newtypes (no bare `Uuid` crosses a boundary).
//! - [`error`] — the canonical [`Error`] model and [`Result`] alias.
//! - [`clock`] — the [`Clock`] port so time-dependent logic (SLA expiry) is testable.
//! - [`events`] — the **frozen event catalog**: the only sanctioned cross-crate contract.
//! - [`traits`] — the [`Space`], [`Component`], and [`Authorization`] service traits.
//!
//! Changing anything here after freeze requires an ADR (PLAN.md section 5.3).

pub mod clock;
pub mod error;
pub mod events;
pub mod ids;
pub mod traits;

pub use clock::{Clock, SystemClock};
pub use error::{Error, Result};
pub use events::{Event, EventEnvelope, EventTopic};
pub use traits::{Authorization, Component, Space, VerificationLevel};

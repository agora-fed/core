//! # dsoc-budgets
//!
//! Tier 2 crate. Participatory budgeting: projects, costs, and citizen allocation under a
//! ceiling.
//!
//! ## Contract
//! - **Emits:** (none) — `budgets.*` is not part of the frozen
//!   [`dsoc_core::events::Event`] catalog and nothing consumes it, so this crate keeps its
//!   state private and exposes it only through its routes (the same posture as `dsoc-admin`
//!   and `dsoc-debates`). Adding a `budgets.*` variant would be a Tier-0 change requiring an
//!   ADR (PLAN.md section 5.3); deferred until a consumer needs it (YAGNI / ADR-0004).
//! - **Consumes:** (none)
//! - **Owns tables:** `budget`, `budget_project`, `budget_order`, `budget_order_item`
//!   (migration `0350_budgets_tables.sql`).
//!
//! ## The allocation rule
//! A citizen's order allocates to a set of projects; the sum of their costs must never exceed
//! the budget's ceiling. This is a pure domain rule ([`domain::check_allocation`]), enforced
//! server-side in [`service::BudgetService::place_order`] over the stored project costs —
//! never a client-supplied total.
//!
//! This crate talks to the rest of the system ONLY through `dsoc-core` traits, `dsoc-app`
//! state, and the gateway. It never reaches into another crate's internals and never depends
//! on a peer `dsoc-*` crate (see `DO NOT` in PLAN.md and CONTRIBUTING.md).

#![forbid(unsafe_code)]

pub mod domain;
pub mod http;
pub mod queries;
pub mod service;

pub use domain::{check_allocation, NewBudget, NewProject};
pub use http::routes;
pub use queries::{BudgetRow, OrderRow, ProjectRow};
pub use service::{BudgetService, PlacedOrder};

use dsoc_core::ids::MandateId;
use dsoc_core::traits::Component;

/// Compile-time marker proving the crate name is wired into the workspace.
pub const CRATE_NAME: &str = "dsoc-budgets";

/// The stable machine name of this participation component.
pub const COMPONENT_KIND: &str = "budgets";

/// The `budgets` participation component (participatory budgeting under a ceiling). Implements
/// [`dsoc_core::traits::Component`] so a space can mount it. A budget is not directed at a
/// mandate (it does not drive the consequence loop), so [`Component::directed_mandate`] is
/// always `None`.
#[derive(Debug, Clone, Copy, Default)]
pub struct BudgetsComponent;

impl Component for BudgetsComponent {
    fn kind(&self) -> &'static str {
        COMPONENT_KIND
    }

    fn directed_mandate(&self) -> Option<MandateId> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_stable() {
        // Guards against accidental rename that would break wiring.
        assert_eq!(CRATE_NAME, "dsoc-budgets");
    }

    #[test]
    fn component_kind_is_budgets_and_undirected() {
        let component = BudgetsComponent;
        assert_eq!(component.kind(), "budgets");
        assert!(
            component.directed_mandate().is_none(),
            "a budget does not direct the consequence loop"
        );
    }
}

//! Pure participatory-budgeting domain: boundary validation for a new budget and project,
//! and the **allocation rule** — the sum of an order's selected project costs must never
//! exceed the budget ceiling. No `sqlx`, no `axum`: everything here is synchronous,
//! side-effect-free, and exhaustively unit-tested (TESTING.md unit layer), which is where
//! the crate earns its coverage.

use dsoc_core::{Error, Result};

/// Maximum length of a budget or project title (Portuguese civic content).
pub const MAX_TITLE_LEN: usize = 200;

/// A validated budget-creation request (title + spending ceiling checked at the boundary).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewBudget {
    /// Title of the budgeting round.
    pub title: String,
    /// The spending ceiling, in cents. Strictly positive.
    pub ceiling_cents: i64,
}

impl NewBudget {
    /// Validate and normalise raw create input: trim the title, require it non-empty and
    /// within [`MAX_TITLE_LEN`], and require a strictly-positive ceiling.
    ///
    /// # Errors
    /// [`Error::Validation`] when the title is empty/over-long or the ceiling is `<= 0`.
    pub fn validate(title: &str, ceiling_cents: i64) -> Result<Self> {
        let title = validate_title(title)?;
        if ceiling_cents <= 0 {
            return Err(Error::Validation(
                "budget ceiling must be a positive amount of cents".to_string(),
            ));
        }
        Ok(Self {
            title,
            ceiling_cents,
        })
    }
}

/// A validated project-creation request (title + cost checked at the boundary).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewProject {
    /// Title of the fundable project.
    pub title: String,
    /// The project's cost, in cents. Strictly positive.
    pub cost_cents: i64,
}

impl NewProject {
    /// Validate and normalise raw create input: trim the title, require it non-empty and
    /// within [`MAX_TITLE_LEN`], and require a strictly-positive cost.
    ///
    /// # Errors
    /// [`Error::Validation`] when the title is empty/over-long or the cost is `<= 0`.
    pub fn validate(title: &str, cost_cents: i64) -> Result<Self> {
        let title = validate_title(title)?;
        if cost_cents <= 0 {
            return Err(Error::Validation(
                "project cost must be a positive amount of cents".to_string(),
            ));
        }
        Ok(Self { title, cost_cents })
    }
}

/// Trim and bound a title shared by budgets and projects.
fn validate_title(title: &str) -> Result<String> {
    let title = title.trim();
    if title.is_empty() {
        return Err(Error::Validation("title must not be empty".to_string()));
    }
    if title.chars().count() > MAX_TITLE_LEN {
        return Err(Error::Validation(format!(
            "title must be at most {MAX_TITLE_LEN} characters"
        )));
    }
    Ok(title.to_string())
}

/// The core domain rule of participatory budgeting: a citizen's order allocates to a
/// non-empty set of projects, and the **sum of their costs must not exceed the ceiling**.
///
/// Returns the total allocated (in cents) when the order is admissible.
///
/// # Errors
/// - [`Error::Validation`] if the order selects no projects, or the running total overflows
///   `i64` (a degenerate/abusive input, never a normal civic order).
/// - [`Error::Conflict`] if the total strictly exceeds `ceiling_cents` — the allocation rule
///   is violated.
pub fn check_allocation(costs: &[i64], ceiling_cents: i64) -> Result<i64> {
    if costs.is_empty() {
        return Err(Error::Validation(
            "an order must allocate to at least one project".to_string(),
        ));
    }
    let mut total: i64 = 0;
    for &cost in costs {
        total = total.checked_add(cost).ok_or_else(|| {
            Error::Validation("allocation total overflows the supported range".to_string())
        })?;
    }
    if total > ceiling_cents {
        return Err(Error::Conflict(format!(
            "allocation of {total} cents exceeds the budget ceiling of {ceiling_cents} cents"
        )));
    }
    Ok(total)
}

/// Clamp a requested page size into `1..=max`. Keeps list reads bounded (PLAN.md: no
/// unbounded reads).
#[must_use]
pub fn clamp_limit(limit: i64, max: i64) -> i64 {
    limit.clamp(1, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_budget_trims_and_accepts_positive_ceiling() {
        let b = NewBudget::validate("  Orçamento 2026  ", 1_000_000).unwrap();
        assert_eq!(b.title, "Orçamento 2026");
        assert_eq!(b.ceiling_cents, 1_000_000);
    }

    #[test]
    fn new_budget_rejects_blank_title() {
        let err = NewBudget::validate("   ", 100).unwrap_err();
        assert_eq!(err.code(), "invalid_input");
    }

    #[test]
    fn new_budget_rejects_overlong_title() {
        let long = "a".repeat(MAX_TITLE_LEN + 1);
        assert_eq!(
            NewBudget::validate(&long, 100).unwrap_err().code(),
            "invalid_input"
        );
    }

    #[test]
    fn new_budget_accepts_max_length_title() {
        let max = "a".repeat(MAX_TITLE_LEN);
        assert!(NewBudget::validate(&max, 100).is_ok());
    }

    #[test]
    fn new_budget_rejects_non_positive_ceiling() {
        for c in [0, -1, i64::MIN] {
            assert_eq!(
                NewBudget::validate("title", c).unwrap_err().code(),
                "invalid_input"
            );
        }
    }

    #[test]
    fn new_project_trims_and_accepts_positive_cost() {
        let p = NewProject::validate("  Praça nova ", 50_000).unwrap();
        assert_eq!(p.title, "Praça nova");
        assert_eq!(p.cost_cents, 50_000);
    }

    #[test]
    fn new_project_rejects_blank_title() {
        assert_eq!(
            NewProject::validate("  ", 10).unwrap_err().code(),
            "invalid_input"
        );
    }

    #[test]
    fn new_project_rejects_non_positive_cost() {
        for c in [0, -100] {
            assert_eq!(
                NewProject::validate("title", c).unwrap_err().code(),
                "invalid_input"
            );
        }
    }

    #[test]
    fn allocation_within_ceiling_returns_total() {
        let total = check_allocation(&[300, 400, 300], 1_000).unwrap();
        assert_eq!(
            total, 1_000,
            "an allocation exactly at the ceiling is allowed"
        );
    }

    #[test]
    fn allocation_below_ceiling_is_allowed() {
        assert_eq!(check_allocation(&[100, 250], 1_000).unwrap(), 350);
    }

    #[test]
    fn allocation_exceeding_ceiling_is_conflict() {
        let err = check_allocation(&[600, 600], 1_000).unwrap_err();
        assert_eq!(err.code(), "conflict");
    }

    #[test]
    fn allocation_one_cent_over_ceiling_is_conflict() {
        let err = check_allocation(&[1_001], 1_000).unwrap_err();
        assert_eq!(err.code(), "conflict");
    }

    #[test]
    fn allocation_requires_at_least_one_project() {
        let err = check_allocation(&[], 1_000).unwrap_err();
        assert_eq!(err.code(), "invalid_input");
    }

    #[test]
    fn allocation_detects_overflow_as_validation() {
        let err = check_allocation(&[i64::MAX, 1], i64::MAX).unwrap_err();
        assert_eq!(err.code(), "invalid_input");
    }

    #[test]
    fn allocation_single_project_at_ceiling() {
        assert_eq!(check_allocation(&[1_000], 1_000).unwrap(), 1_000);
    }

    #[test]
    fn clamp_limit_bounds_page_size() {
        assert_eq!(clamp_limit(0, 100), 1);
        assert_eq!(clamp_limit(-10, 100), 1);
        assert_eq!(clamp_limit(20, 100), 20);
        assert_eq!(clamp_limit(10_000, 100), 100);
    }
}

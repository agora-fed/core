//! Pure proposal domain: status lifecycle, input validation, and the **threshold-crossing policy**
//! that triggers the consequence loop — the accountability thesis. No `sqlx`, no `axum`: everything
//! here is unit-testable in isolation, which is where this crate earns its coverage (TESTING.md).

use dsoc_core::{Error, Result};
use uuid::Uuid;

/// Maximum length of a proposal title (defensive bound at the boundary).
pub const MAX_TITLE_LEN: usize = 200;
/// Maximum length of a proposal body.
pub const MAX_BODY_LEN: usize = 20_000;
/// Maximum recipients per proposal (primary + co-recipients). Stops a proposal from
/// becoming a mass mailing to all 513 offices of the lower house at once.
pub const MAX_PROPOSAL_TARGETS: usize = 10;

/// The proposal lifecycle status. The authoritative facts are the dedicated timestamp/cluster
/// columns (`published_at`, `cluster_id`, `threshold_crossed_at`); `status` is the human-facing
/// summary of the most recent significant transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalStatus {
    /// Freshly created; not yet published or clustered.
    Draft,
    /// Cleared moderation and published.
    Published,
    /// Merged into a consensus cluster (near-duplicates folded into one signal).
    Clustered,
}

impl ProposalStatus {
    /// The stable text stored in the `proposal.status` column (matches the SQL CHECK constraint).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            ProposalStatus::Draft => "draft",
            ProposalStatus::Published => "published",
            ProposalStatus::Clustered => "clustered",
        }
    }

    /// Parse the stored text back into the enum.
    ///
    /// # Errors
    /// Returns [`Error::Storage`] for an unrecognised value (a schema/CHECK drift bug).
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "draft" => Ok(ProposalStatus::Draft),
            "published" => Ok(ProposalStatus::Published),
            "clustered" => Ok(ProposalStatus::Clustered),
            other => Err(Error::Storage(
                format!("unknown proposal status: {other}").into(),
            )),
        }
    }
}

/// A validated proposal creation request (title/body/threshold checked at the boundary).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewProposal {
    /// Title (Portuguese civic content).
    pub title: String,
    /// Body / description.
    pub body: String,
    /// Support count at which the consequence loop fires. Must be positive.
    pub threshold: i32,
}

impl NewProposal {
    /// Validate and normalise raw create input (trim, non-empty, length & threshold bounds).
    ///
    /// # Errors
    /// [`Error::Validation`] when the title/body are empty or over length, or the threshold is not
    /// strictly positive.
    pub fn validate(title: &str, body: &str, threshold: i32) -> Result<Self> {
        let title = title.trim();
        let body = body.trim();
        if title.is_empty() {
            return Err(Error::Validation("title must not be empty".to_string()));
        }
        if body.is_empty() {
            return Err(Error::Validation("body must not be empty".to_string()));
        }
        if title.chars().count() > MAX_TITLE_LEN {
            return Err(Error::Validation(format!(
                "title must be at most {MAX_TITLE_LEN} characters"
            )));
        }
        if body.chars().count() > MAX_BODY_LEN {
            return Err(Error::Validation(format!(
                "body must be at most {MAX_BODY_LEN} characters"
            )));
        }
        if threshold <= 0 {
            return Err(Error::Validation(
                "threshold must be a positive integer".to_string(),
            ));
        }
        Ok(Self {
            title: title.to_string(),
            body: body.to_string(),
            threshold,
        })
    }
}

/// A validated revision edit (append-only history entry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRevision {
    /// New title.
    pub title: String,
    /// New body.
    pub body: String,
}

impl NewRevision {
    /// Validate and normalise a revision edit.
    ///
    /// # Errors
    /// [`Error::Validation`] when the title/body are empty or over length.
    pub fn validate(title: &str, body: &str) -> Result<Self> {
        // Reuse the create-time bounds; a revision is a fresh title+body without a threshold.
        let np = NewProposal::validate(title, body, 1)?;
        Ok(Self {
            title: np.title,
            body: np.body,
        })
    }
}

/// Normalise the full target list of a proposal: the primary mandate first, then the
/// additional ones in the order given, with duplicates (including of the primary) dropped.
///
/// # Errors
/// [`Error::Validation`] when the deduplicated list exceeds [`MAX_PROPOSAL_TARGETS`].
pub fn normalize_targets(primary: Uuid, additional: &[Uuid]) -> Result<Vec<Uuid>> {
    let mut targets = Vec::with_capacity(1 + additional.len());
    targets.push(primary);
    for id in additional {
        if !targets.contains(id) {
            targets.push(*id);
        }
    }
    if targets.len() > MAX_PROPOSAL_TARGETS {
        return Err(Error::Validation(format!(
            "a proposta pode ter no máximo {MAX_PROPOSAL_TARGETS} destinatários"
        )));
    }
    Ok(targets)
}

/// The **threshold-crossing policy** — the heart of the accountability thesis.
///
/// A proposal crosses (firing the one consequence signal) **iff** all hold:
/// 1. it belongs to a consensus cluster (`cluster_present`) — this is what folds the noise of many
///    near-duplicate proposals into a single accountable signal (the noise→one-signal thesis);
/// 2. its aggregate support has reached the directed `threshold` (`support_count >= threshold`,
///    boundary inclusive);
/// 3. it has not already crossed (`!already_crossed`) — duplicate vote signals must never re-fire.
///
/// Pure and total: no I/O, no clock. The service applies it under a row lock so the database
/// decision matches this policy exactly.
#[must_use]
pub fn crossing_fires(
    cluster_present: bool,
    support_count: i64,
    threshold: i32,
    already_crossed: bool,
) -> bool {
    cluster_present && !already_crossed && support_count >= i64::from(threshold)
}

/// Fold an incoming aggregate tally into the stored one. Tallies are delivered at-least-once and may
/// arrive out of order, so we keep the **maximum** ever seen: a stale or replayed signal can never
/// lower a proposal's support (monotonic), which keeps a crossing from being "un-crossed".
#[must_use]
pub fn merge_support(current: i64, incoming: i64) -> i64 {
    current.max(incoming)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_round_trips_through_text() {
        for s in [
            ProposalStatus::Draft,
            ProposalStatus::Published,
            ProposalStatus::Clustered,
        ] {
            assert_eq!(ProposalStatus::parse(s.as_str()).unwrap(), s);
        }
    }

    #[test]
    fn status_parse_rejects_unknown() {
        assert!(ProposalStatus::parse("bogus").is_err());
    }

    #[test]
    fn validate_trims_and_accepts_content() {
        let np = NewProposal::validate("  Mais creches  ", "  no bairro centro ", 100).unwrap();
        assert_eq!(np.title, "Mais creches");
        assert_eq!(np.body, "no bairro centro");
        assert_eq!(np.threshold, 100);
    }

    #[test]
    fn validate_rejects_blank_title() {
        let err = NewProposal::validate("   ", "body", 10).unwrap_err();
        assert_eq!(err.code(), "invalid_input");
    }

    #[test]
    fn validate_rejects_blank_body() {
        assert!(NewProposal::validate("title", "  ", 10).is_err());
    }

    #[test]
    fn validate_rejects_nonpositive_threshold() {
        assert!(NewProposal::validate("t", "b", 0).is_err());
        assert!(NewProposal::validate("t", "b", -5).is_err());
    }

    #[test]
    fn validate_rejects_overlong_title() {
        let long = "a".repeat(MAX_TITLE_LEN + 1);
        assert!(NewProposal::validate(&long, "b", 1).is_err());
    }

    #[test]
    fn validate_rejects_overlong_body() {
        let long = "a".repeat(MAX_BODY_LEN + 1);
        assert!(NewProposal::validate("t", &long, 1).is_err());
    }

    #[test]
    fn revision_validates_like_a_proposal() {
        let r = NewRevision::validate(" Novo título ", " novo corpo ").unwrap();
        assert_eq!(r.title, "Novo título");
        assert_eq!(r.body, "novo corpo");
        assert!(NewRevision::validate("", "b").is_err());
    }

    // --- the thesis: threshold-crossing policy ---

    #[test]
    fn crossing_fires_when_clustered_and_at_threshold() {
        assert!(crossing_fires(true, 100, 100, false));
    }

    #[test]
    fn crossing_fires_when_clustered_and_over_threshold() {
        assert!(crossing_fires(true, 250, 100, false));
    }

    #[test]
    fn crossing_does_not_fire_below_threshold() {
        assert!(!crossing_fires(true, 99, 100, false));
    }

    #[test]
    fn unclustered_proposal_cannot_cross_even_when_over_threshold() {
        // The thesis guard: no cluster means many near-duplicate proposals have not been folded into
        // one signal, so the consequence loop must not fire.
        assert!(!crossing_fires(false, 10_000, 100, false));
    }

    #[test]
    fn crossing_is_idempotent_once_already_crossed() {
        // Duplicate at-least-once vote signals must never re-fire the accountability signal.
        assert!(!crossing_fires(true, 100, 100, true));
        assert!(!crossing_fires(true, 999, 1, true));
    }

    // --- multi-recipient (0537) ---

    #[test]
    fn normalize_targets_keeps_primary_first_and_dedupes() {
        let primary = Uuid::now_v7();
        let extra = Uuid::now_v7();
        let targets = normalize_targets(primary, &[extra, primary, extra]).unwrap();
        assert_eq!(targets, vec![primary, extra]);
    }

    #[test]
    fn normalize_targets_accepts_primary_only() {
        let primary = Uuid::now_v7();
        assert_eq!(normalize_targets(primary, &[]).unwrap(), vec![primary]);
    }

    #[test]
    fn normalize_targets_rejects_over_cap() {
        let primary = Uuid::now_v7();
        let extras: Vec<Uuid> = (0..MAX_PROPOSAL_TARGETS).map(|_| Uuid::now_v7()).collect();
        let err = normalize_targets(primary, &extras).unwrap_err();
        assert_eq!(err.code(), "invalid_input");
    }

    #[test]
    fn normalize_targets_accepts_exactly_cap() {
        let primary = Uuid::now_v7();
        let extras: Vec<Uuid> = (0..MAX_PROPOSAL_TARGETS - 1)
            .map(|_| Uuid::now_v7())
            .collect();
        let targets = normalize_targets(primary, &extras).unwrap();
        assert_eq!(targets.len(), MAX_PROPOSAL_TARGETS);
    }

    #[test]
    fn merge_support_is_monotonic() {
        assert_eq!(merge_support(10, 25), 25);
        assert_eq!(
            merge_support(25, 10),
            25,
            "a stale replay cannot lower support"
        );
        assert_eq!(merge_support(0, 0), 0);
    }
}

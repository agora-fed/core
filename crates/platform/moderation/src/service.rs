//! The moderation service: orchestrates the pure domain, the `sqlx` queries, the
//! injected [`Clock`], and the [`EventBus`]. It maps storage failures onto the canonical
//! [`dsoc_core::Error`] and guarantees that **every** evaluation persists an auditable
//! decision before its event is emitted (decisions are never silently dropped).

use std::sync::Arc;

use dsoc_core::clock::Clock;
use dsoc_core::error::{Error, Result};
use dsoc_core::ids::{CommentId, OrgId, ProposalId};
use dsoc_core::traits::EventBus;
use dsoc_db::Db;
use uuid::Uuid;

use crate::domain::{self, Appeal, AppealStatus, Decision, Rule, RuleAction, RuleKind, TargetKind};
use crate::events;
use crate::queries::{self, Cursor};

/// Upper bound on free-text fields (keyword pattern, appeal reason).
const MAX_TEXT_LEN: usize = 2_000;
/// Safety cap on the number of rules pulled for a single evaluation.
const MAX_RULES_FOR_EVAL: i64 = 1_000;
/// Default page size for list endpoints.
pub const DEFAULT_PAGE_LIMIT: i64 = 50;
/// Hard cap on page size to keep reads bounded.
pub const MAX_PAGE_LIMIT: i64 = 200;

/// What a decision is about. Carries the parent proposal so the emitted moderation event
/// (which is keyed by `ProposalId` in the frozen catalog) is always well-defined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModerationTarget {
    /// A proposal target; the event references the proposal itself.
    Proposal(ProposalId),
    /// A comment target; the event references the comment's parent proposal.
    Comment {
        /// The comment under moderation.
        comment: CommentId,
        /// Its parent proposal (the moderation event subject).
        proposal: ProposalId,
    },
}

impl ModerationTarget {
    fn kind(self) -> TargetKind {
        match self {
            ModerationTarget::Proposal(_) => TargetKind::Proposal,
            ModerationTarget::Comment { .. } => TargetKind::Comment,
        }
    }

    fn target_id(self) -> Uuid {
        match self {
            ModerationTarget::Proposal(p) => p.as_uuid(),
            ModerationTarget::Comment { comment, .. } => comment.as_uuid(),
        }
    }

    /// The proposal the emitted moderation event is keyed by.
    fn event_proposal(self) -> ProposalId {
        match self {
            ModerationTarget::Proposal(p) => p,
            ModerationTarget::Comment { proposal, .. } => proposal,
        }
    }
}

/// The moderation service. Cheap to construct per request from `AppState`.
pub struct ModerationService {
    db: Db,
    clock: Arc<dyn Clock>,
    bus: Arc<dyn EventBus>,
}

impl std::fmt::Debug for ModerationService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModerationService").finish_non_exhaustive()
    }
}

impl ModerationService {
    /// Build a service from its injected ports.
    #[must_use]
    pub fn new(db: Db, clock: Arc<dyn Clock>, bus: Arc<dyn EventBus>) -> Self {
        Self { db, clock, bus }
    }

    /// Create a moderation rule for an organization, validating its parameter.
    ///
    /// # Errors
    /// [`Error::Validation`] for an empty/oversized or malformed pattern;
    /// [`Error::Conflict`]/[`Error::Storage`] on persistence failure.
    pub async fn create_rule(
        &self,
        org: OrgId,
        kind: RuleKind,
        pattern: &str,
        action: RuleAction,
    ) -> Result<Rule> {
        let pattern =
            domain::validate_nonempty("pattern", pattern, MAX_TEXT_LEN).map_err(validation)?;
        if kind == RuleKind::CapsRatio && !is_valid_ratio(&pattern) {
            return Err(Error::Validation(
                "caps_ratio pattern must be a number in 0.0..=1.0".to_owned(),
            ));
        }
        let rule = Rule {
            id: Uuid::now_v7(),
            org_id: org.as_uuid(),
            kind,
            pattern,
            action,
            created_at: self.clock.now(),
        };
        queries::insert_rule(&self.db, &rule)
            .await
            .map_err(map_storage)?;
        Ok(rule)
    }

    /// Evaluate `content` for a target against the organization's ruleset, persist the
    /// auditable decision, and emit `moderation.flagged` or `moderation.cleared`.
    ///
    /// The decision row is written **before** the event is published, so a flagged or
    /// cleared verdict is always recoverable from the audit log even if delivery fails.
    ///
    /// # Errors
    /// [`Error::Storage`] (or a mapped variant) on persistence or publish failure.
    pub async fn evaluate(
        &self,
        org: OrgId,
        target: ModerationTarget,
        content: &str,
    ) -> Result<Decision> {
        let rules = queries::fetch_rules_for_eval(&self.db, org.as_uuid(), MAX_RULES_FOR_EVAL)
            .await
            .map_err(map_storage)?;
        let matched = domain::first_match(&rules, content);
        let outcome = domain::outcome_for(matched);

        let decision = Decision {
            id: Uuid::now_v7(),
            org_id: org.as_uuid(),
            target_kind: target.kind(),
            target_id: target.target_id(),
            rule_id: matched.map(|r| r.id),
            outcome,
            created_at: self.clock.now(),
        };
        // Persist first: the audit record must exist regardless of event delivery.
        queries::insert_decision(&self.db, &decision)
            .await
            .map_err(map_storage)?;

        events::emit_decision(
            self.bus.as_ref(),
            self.clock.as_ref(),
            org,
            target.event_proposal(),
            outcome,
        )
        .await?;

        Ok(decision)
    }

    /// Fetch a single decision (the audit record).
    ///
    /// # Errors
    /// [`Error::NotFound`] when absent; [`Error::Storage`] otherwise.
    pub async fn get_decision(&self, id: Uuid) -> Result<Decision> {
        queries::get_decision(&self.db, id)
            .await
            .map_err(map_storage)
    }

    /// List an organization's decisions newest-first (keyset-paginated audit log).
    ///
    /// # Errors
    /// [`Error::Storage`] on a storage failure.
    pub async fn list_decisions(
        &self,
        org: OrgId,
        cursor: Option<Cursor>,
        limit: i64,
    ) -> Result<Vec<Decision>> {
        queries::list_decisions(&self.db, org.as_uuid(), cursor, clamp_limit(limit))
            .await
            .map_err(map_storage)
    }

    /// List an organization's rules newest-first (keyset-paginated).
    ///
    /// # Errors
    /// [`Error::Storage`] on a storage failure.
    pub async fn list_rules(
        &self,
        org: OrgId,
        cursor: Option<Cursor>,
        limit: i64,
    ) -> Result<Vec<Rule>> {
        queries::list_rules(&self.db, org.as_uuid(), cursor, clamp_limit(limit))
            .await
            .map_err(map_storage)
    }

    /// File an appeal against a decision. The decision must exist (mapped to
    /// [`Error::NotFound`]); the appeal starts in the `open` state.
    ///
    /// # Errors
    /// [`Error::Validation`] for an empty/oversized reason; [`Error::NotFound`] for an
    /// unknown decision; [`Error::Storage`] otherwise.
    pub async fn file_appeal(&self, decision_id: Uuid, reason: &str) -> Result<Appeal> {
        let reason =
            domain::validate_nonempty("reason", reason, MAX_TEXT_LEN).map_err(validation)?;
        // Surface a clean NotFound rather than a raw FK violation.
        self.get_decision(decision_id).await?;
        let now = self.clock.now();
        let appeal = Appeal {
            id: Uuid::now_v7(),
            decision_id,
            reason,
            status: AppealStatus::Open,
            created_at: now,
            updated_at: now,
        };
        queries::insert_appeal(&self.db, &appeal)
            .await
            .map_err(map_storage)?;
        Ok(appeal)
    }

    /// Resolve an appeal by moving it to `granted` or `denied`. The domain state machine
    /// rejects re-deciding an already resolved appeal ([`Error::Conflict`]).
    ///
    /// # Errors
    /// [`Error::NotFound`] for an unknown appeal; [`Error::Conflict`] for an illegal
    /// transition; [`Error::Storage`] otherwise.
    pub async fn resolve_appeal(&self, appeal_id: Uuid, to: AppealStatus) -> Result<Appeal> {
        let appeal = queries::get_appeal(&self.db, appeal_id)
            .await
            .map_err(map_storage)?;
        let next = appeal
            .status
            .transition(to)
            .map_err(|e| Error::Conflict(e.to_string()))?;
        queries::update_appeal_status(&self.db, appeal_id, next, self.clock.now())
            .await
            .map_err(map_storage)
    }
}

fn is_valid_ratio(pattern: &str) -> bool {
    matches!(pattern.trim().parse::<f64>(), Ok(v) if (0.0..=1.0).contains(&v))
}

fn clamp_limit(limit: i64) -> i64 {
    limit.clamp(1, MAX_PAGE_LIMIT)
}

fn validation(err: domain::ParseError) -> Error {
    Error::Validation(err.to_string())
}

/// Map a storage-layer [`queries::Error`] onto the canonical [`dsoc_core::Error`]
/// (sqlx `RowNotFound` -> `NotFound`; unique violation -> `Conflict`; bad input ->
/// `Validation`; everything else -> `Storage`).
fn map_storage(err: queries::Error) -> Error {
    match err {
        queries::Error::Db(sqlx::Error::RowNotFound) => {
            Error::NotFound("moderation entity not found".to_owned())
        }
        queries::Error::Db(sqlx::Error::Database(db_err)) => match db_err.code().as_deref() {
            // unique_violation
            Some("23505") => Error::Conflict("moderation entity already exists".to_owned()),
            // foreign_key / check / not_null violation -> bad input
            Some("23503" | "23514" | "23502") => {
                Error::Validation("moderation input violates a constraint".to_owned())
            }
            _ => Error::Storage(Box::new(db_err)),
        },
        queries::Error::Db(other) => Error::Storage(Box::new(other)),
        // A corrupt stored value is an internal integrity failure, not a client error.
        queries::Error::Decode(parse) => Error::Storage(Box::new(parse)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_limit_bounds_page_size() {
        assert_eq!(clamp_limit(0), 1);
        assert_eq!(clamp_limit(-5), 1);
        assert_eq!(clamp_limit(50), 50);
        assert_eq!(clamp_limit(10_000), MAX_PAGE_LIMIT);
    }

    #[test]
    fn ratio_validation_accepts_only_unit_interval() {
        assert!(is_valid_ratio("0.0"));
        assert!(is_valid_ratio("1.0"));
        assert!(is_valid_ratio(" 0.75 "));
        assert!(!is_valid_ratio("1.5"));
        assert!(!is_valid_ratio("-0.1"));
        assert!(!is_valid_ratio("abc"));
    }

    #[test]
    fn row_not_found_maps_to_not_found() {
        let mapped = map_storage(queries::Error::Db(sqlx::Error::RowNotFound));
        assert!(matches!(mapped, Error::NotFound(_)));
        assert_eq!(mapped.code(), "not_found");
    }

    #[test]
    fn decode_error_maps_to_storage() {
        let parse = domain::ParseError {
            field: "kind",
            value: "garbage".to_owned(),
        };
        let mapped = map_storage(queries::Error::Decode(parse));
        assert_eq!(mapped.code(), "storage_error");
    }

    #[test]
    fn validation_helper_maps_parse_error() {
        let parse = domain::ParseError {
            field: "reason",
            value: String::new(),
        };
        assert!(matches!(validation(parse), Error::Validation(_)));
    }

    #[test]
    fn target_projects_kind_id_and_event_proposal() {
        let p = ProposalId::new();
        let prop_target = ModerationTarget::Proposal(p);
        assert_eq!(prop_target.kind(), TargetKind::Proposal);
        assert_eq!(prop_target.target_id(), p.as_uuid());
        assert_eq!(prop_target.event_proposal(), p);

        let c = CommentId::new();
        let comment_target = ModerationTarget::Comment {
            comment: c,
            proposal: p,
        };
        assert_eq!(comment_target.kind(), TargetKind::Comment);
        assert_eq!(comment_target.target_id(), c.as_uuid());
        // The event is keyed by the parent proposal, not the comment.
        assert_eq!(comment_target.event_proposal(), p);
    }
}

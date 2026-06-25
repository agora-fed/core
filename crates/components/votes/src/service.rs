//! The vote service: holds the `Db` pool and the injected `Arc<dyn Clock>` (ADR-0004 wiring).
//! It records support signals and serves the privacy-safe aggregate. Events are emitted through
//! the **transactional outbox** (ADR-0006), so no `Arc<dyn EventBus>` is needed here. All `sqlx`
//! failures map onto the canonical [`dsoc_core::Error`] model.
//!
//! ## Privacy boundary (LGPD)
//! [`VoteService::cast`] is the only method that touches the protected `votes_vote` linkage. The
//! official-facing methods [`VoteService::tally`] and [`VoteService::list_tallies`] call **only**
//! the aggregate query functions, so a citizen id cannot leak into an official response.

use std::sync::Arc;

use dsoc_core::ids::{CitizenId, OrgId, ProposalId, VoteId};
use dsoc_core::{Clock, Error, Result};
use dsoc_db::Db;

use crate::domain::{self, TallyView};
use crate::events;
use crate::queries::{self, TallyRow};

/// Hard cap on an aggregate-listing page (unbounded reads must be bounded — PLAN.md).
const MAX_PAGE: i64 = 100;

/// The outcome of recording a support signal, returned to the voter (their own data).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CastReceipt {
    /// The opaque id of the recorded vote.
    pub vote: VoteId,
    /// The proposal supported.
    pub proposal: ProposalId,
    /// The proposal's new aggregate support count after this cast.
    pub support_count: u64,
}

/// Support-signal service for proposals.
#[derive(Clone)]
pub struct VoteService {
    db: Db,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for VoteService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VoteService").finish_non_exhaustive()
    }
}

/// Map a `sqlx` failure onto the canonical, public-safe error model.
fn map_sqlx(error: sqlx::Error) -> Error {
    match error {
        sqlx::Error::RowNotFound => Error::NotFound("entity not found".to_owned()),
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            Error::Conflict("entity already exists".to_owned())
        }
        other => Error::Storage(Box::new(other)),
    }
}

impl VoteService {
    /// Construct the service from its injected collaborators.
    #[must_use]
    pub fn new(db: Db, clock: Arc<dyn Clock>) -> Self {
        Self { db, clock }
    }

    /// Build a service from the shared application state.
    #[must_use]
    pub fn from_state(state: &dsoc_app::AppState) -> Self {
        Self::new(state.db.clone(), state.clock.clone())
    }

    /// Record a citizen's support signal for a proposal, increment the aggregate tally, and emit
    /// `votes.cast` + `votes.tally.updated` — **all in one transaction** via the transactional
    /// outbox (ADR-0006). There is no post-commit window where the vote persisted but its event was
    /// lost, and a duplicate cast cannot suppress an already-emitted signal.
    ///
    /// A second cast by the same citizen for the same proposal is rejected with
    /// [`Error::Conflict`] *before* the tally is touched, so the aggregate is never double-counted.
    ///
    /// Authorization (the caller must be at least email-verified) is enforced by the HTTP layer via
    /// the injected [`dsoc_core::Authorization`] before this method is reached.
    ///
    /// # Errors
    /// - [`Error::Conflict`] when the citizen already supported this proposal.
    /// - [`Error::Storage`] on any other persistence failure.
    pub async fn cast(
        &self,
        org: OrgId,
        proposal: ProposalId,
        citizen: CitizenId,
    ) -> Result<CastReceipt> {
        let now = self.clock.now();
        let vote_id = VoteId::new();

        let mut tx = self.db.begin().await.map_err(map_sqlx)?;

        // Protected linkage write. `None` => the UNIQUE(proposal, citizen) row already exists, i.e.
        // a duplicate cast: roll back so the tally is left untouched, and report Conflict.
        let stored = queries::insert_vote(
            &mut *tx,
            vote_id.as_uuid(),
            org.as_uuid(),
            proposal.as_uuid(),
            citizen.as_uuid(),
            now,
        )
        .await
        .map_err(map_sqlx)?;
        let Some(stored_id) = stored else {
            tx.rollback().await.map_err(map_sqlx)?;
            return Err(Error::Conflict(
                "citizen has already supported this proposal".to_owned(),
            ));
        };
        // Use the id PostgreSQL actually stored (RETURNING), never a phantom pre-generated value.
        let vote = VoteId::from_uuid(stored_id);

        let new_count = queries::upsert_tally(&mut *tx, proposal.as_uuid(), now)
            .await
            .map_err(map_sqlx)?;
        let support_count = domain::normalize_support(new_count);

        // Transactional outbox: both events commit atomically with the writes above.
        let cast_env = events::vote_cast_envelope(self.clock.as_ref(), org, vote, proposal);
        dsoc_db::outbox::publish_tx(&mut *tx, &cast_env)
            .await
            .map_err(map_sqlx)?;
        let tally_env =
            events::tally_updated_envelope(self.clock.as_ref(), org, proposal, support_count);
        dsoc_db::outbox::publish_tx(&mut *tx, &tally_env)
            .await
            .map_err(map_sqlx)?;

        tx.commit().await.map_err(map_sqlx)?;

        Ok(CastReceipt {
            vote,
            proposal,
            support_count,
        })
    }

    /// Read the privacy-safe aggregate for a proposal (official-facing). Reads ONLY the tally
    /// table; no citizen linkage is touched.
    ///
    /// # Errors
    /// [`Error::NotFound`] if the proposal has no support yet; [`Error::Storage`] on failure.
    pub async fn tally(&self, proposal: ProposalId) -> Result<TallyView> {
        let row = queries::tally(&self.db, proposal.as_uuid())
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| Error::NotFound("no tally for proposal".to_owned()))?;
        Ok(view_from_row(row))
    }

    /// Keyset-paginated browse of aggregates (official-facing), ascending by proposal id. Returns
    /// the page plus the total aggregate-row count for pagination metadata.
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn list_tallies(
        &self,
        after: Option<ProposalId>,
        limit: i64,
    ) -> Result<(Vec<TallyView>, i64)> {
        let bounded = limit.clamp(1, MAX_PAGE);
        let rows = queries::list_tallies(&self.db, after.map(|p| p.as_uuid()), bounded)
            .await
            .map_err(map_sqlx)?;
        let total = queries::count_tallies(&self.db).await.map_err(map_sqlx)?;
        let views = rows.into_iter().map(view_from_row).collect();
        Ok((views, total))
    }

    /// Handle an inbound cross-crate event (idempotent). `votes` consumes nothing, so this is
    /// always a no-op returning `false`. Present for symmetry with the wiring conventions.
    ///
    /// # Errors
    /// Propagates any error from the consume handler (none today).
    pub fn consume(&self, envelope: &dsoc_core::events::EventEnvelope) -> Result<bool> {
        events::on_event(envelope)
    }
}

/// Map an aggregate row to the privacy-safe domain view (clamping a corrupt count to zero).
fn view_from_row(row: TallyRow) -> TallyView {
    TallyView {
        proposal: ProposalId::from_uuid(row.proposal_id),
        support_count: domain::normalize_support(row.support_count),
        updated_at: row.updated_at,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use chrono::{DateTime, Utc};

    #[test]
    fn map_sqlx_row_not_found_is_not_found() {
        assert_eq!(map_sqlx(sqlx::Error::RowNotFound).code(), "not_found");
    }

    #[test]
    fn map_sqlx_protocol_error_is_storage_and_hides_detail() {
        let mapped = map_sqlx(sqlx::Error::Protocol("secret".into()));
        assert_eq!(mapped.code(), "storage_error");
        assert_eq!(mapped.to_string(), "storage error");
    }

    #[test]
    fn view_from_row_clamps_and_maps() {
        let at = DateTime::parse_from_rfc3339("2026-06-25T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let pid = ProposalId::new();
        let view = view_from_row(TallyRow {
            proposal_id: pid.as_uuid(),
            support_count: -5,
            updated_at: at,
        });
        assert_eq!(view.proposal, pid);
        assert_eq!(view.support_count, 0);
        assert_eq!(view.updated_at, at);
    }
}

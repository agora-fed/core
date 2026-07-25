//! The proposals service: holds the pool and the injected [`Clock`]. It creates proposals, appends
//! revisions, and folds the inbound event stream (consensus links, moderation clearance, vote
//! tallies) into proposal state — firing the **consequence loop** when support crosses the directed
//! threshold *while clustered*. Every mutation emits its catalog event through the **transactional
//! outbox** (ADR-0006) inside the same transaction as the write, so the change and the event commit
//! atomically. All failures map onto the canonical [`dsoc_core::Error`].

use std::sync::Arc;

use dsoc_core::events::{Event, EventEnvelope};
use dsoc_core::ids::{CitizenId, ClusterId, EventId, MandateId, OrgId, ProposalId};
use dsoc_core::{Clock, Error, Result};
use dsoc_db::Db;
use uuid::Uuid;

use crate::domain::{crossing_fires, merge_support, NewProposal, NewRevision};
use crate::events;
use crate::queries::{self, ProposalRow, RevisionRow, TargetRow};

/// The outcome of consuming one inbound event. Returned so callers (and tests) can assert exactly
/// what a delivery did without re-reading the database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consumed {
    /// A `consensus.proposal.merged` linked the proposal to a cluster. `crossed` is `true` if the
    /// link also fired the threshold crossing (support was already at/over threshold).
    Linked {
        /// The linked proposal.
        proposal: ProposalId,
        /// The cluster it now belongs to.
        cluster: ClusterId,
        /// Whether the link fired the threshold crossing.
        crossed: bool,
    },
    /// A `moderation.cleared` published the proposal.
    Published {
        /// The published proposal.
        proposal: ProposalId,
        /// Whether this delivery actually emitted (false on an idempotent duplicate).
        emitted: bool,
    },
    /// A `votes.tally.updated` folded a new aggregate support count.
    TallyUpdated {
        /// The proposal whose tally changed.
        proposal: ProposalId,
        /// The stored support count after the monotonic fold.
        support_count: i64,
        /// Whether this delivery fired the threshold crossing.
        crossed: bool,
    },
    /// The envelope was not one this crate acts on, or referenced an unknown proposal (no-op).
    Ignored,
}

/// Proposals service.
#[derive(Clone)]
pub struct ProposalService {
    db: Db,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for ProposalService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProposalService").finish_non_exhaustive()
    }
}

impl ProposalService {
    /// Build a service from an explicit pool and clock (used by tests).
    #[must_use]
    pub fn new(db: Db, clock: Arc<dyn Clock>) -> Self {
        Self { db, clock }
    }

    /// Build a service from the shared application state.
    #[must_use]
    pub fn from_state(state: &dsoc_app::AppState) -> Self {
        Self::new(state.db.clone(), state.clock.clone())
    }

    /// Create a proposal directed at `mandate` and emit `proposals.created` through the outbox,
    /// atomically with the insert.
    ///
    /// # Errors
    /// - [`Error::Conflict`] on a unique/foreign-key violation (e.g. unknown mandate/org).
    /// - [`Error::Storage`] on any other persistence failure.
    pub async fn create(
        &self,
        org: OrgId,
        mandate: MandateId,
        author: Option<CitizenId>,
        new: &NewProposal,
    ) -> Result<ProposalRow> {
        self.create_targets(org, &[mandate], author, new).await
    }

    /// Create a proposal directed at one or more mandates — the first is the PRINCIPAL target
    /// (drives the consequence loop and the legacy `proposal.mandate_id`); every target gets a
    /// `proposal_target` row for per-gabinete delivery. All targets must exist and belong to the
    /// SAME federative sphere (`mandate.sphere`, migration 0203) — a proposal cannot mix, say,
    /// deputados federais with vereadores. The caller has already deduplicated and capped the
    /// list via [`crate::domain::normalize_targets`].
    ///
    /// # Errors
    /// - [`Error::Validation`] when `targets` is empty or spans more than one sphere.
    /// - [`Error::Conflict`] when any target mandate (or the org) does not exist.
    /// - [`Error::Storage`] on any other persistence failure.
    pub async fn create_targets(
        &self,
        org: OrgId,
        targets: &[MandateId],
        author: Option<CitizenId>,
        new: &NewProposal,
    ) -> Result<ProposalRow> {
        let [mandate, ..] = targets else {
            return Err(Error::Validation(
                "a proposta precisa de ao menos um destinatário".to_string(),
            ));
        };
        let mandate = *mandate;
        let now = self.clock.now();
        let proposal = ProposalId::new();
        let target_uuids: Vec<Uuid> = targets.iter().map(|m| m.as_uuid()).collect();

        let mut tx = self.db.begin().await.map_err(map_sqlx)?;

        // Esfera única (0537): todos os destinatários existem e pertencem à mesma esfera.
        let check = queries::check_target_spheres(&mut *tx, &target_uuids)
            .await
            .map_err(map_sqlx)?;
        if check.found != i64::try_from(target_uuids.len()).unwrap_or(i64::MAX) {
            return Err(Error::Conflict(
                "referenced org or mandate does not exist".to_string(),
            ));
        }
        if check.spheres > 1 {
            return Err(Error::Validation(
                "todos os destinatários devem ser da mesma esfera (federal, estadual ou municipal)"
                    .to_string(),
            ));
        }

        let row = queries::insert_proposal(
            &mut *tx,
            proposal.as_uuid(),
            org.as_uuid(),
            mandate.as_uuid(),
            &new.title,
            &new.body,
            new.threshold,
            now,
            author.map(|c| c.as_uuid()),
        )
        .await
        .map_err(map_sqlx)?;

        for target in &target_uuids {
            queries::insert_target(&mut *tx, proposal.as_uuid(), *target, now)
                .await
                .map_err(map_sqlx)?;
        }

        let envelope = events::envelope(
            &self.clock,
            org,
            Event::ProposalCreated { proposal, mandate },
        );
        dsoc_db::outbox::publish_tx(&mut *tx, &envelope)
            .await
            .map_err(map_sqlx)?;
        tx.commit().await.map_err(map_sqlx)?;
        Ok(row)
    }

    /// List a proposal's targets (principal first) with the mandate display data joined.
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn targets(&self, id: ProposalId) -> Result<Vec<TargetRow>> {
        queries::list_targets(&self.db, id.as_uuid())
            .await
            .map_err(map_sqlx)
    }

    /// Fetch a single proposal.
    ///
    /// # Errors
    /// [`Error::NotFound`] when absent; [`Error::Storage`] otherwise.
    pub async fn get(&self, id: ProposalId) -> Result<ProposalRow> {
        queries::get_proposal(&self.db, id.as_uuid())
            .await
            .map_err(map_sqlx)
    }

    /// List an org's proposals with keyset pagination, returning the rows and the total count.
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn list(
        &self,
        org: OrgId,
        after: Option<ProposalId>,
        limit: i64,
    ) -> Result<(Vec<ProposalRow>, i64)> {
        let limit = limit.clamp(1, 100);
        let rows =
            queries::list_proposals(&self.db, org.as_uuid(), after.map(|p| p.as_uuid()), limit)
                .await
                .map_err(map_sqlx)?;
        let total = queries::count_proposals(&self.db, org.as_uuid())
            .await
            .map_err(map_sqlx)?;
        Ok((rows, total))
    }

    /// Append a revision (append-only history) and mirror the new title/body onto the proposal.
    /// Existing revisions are never updated or deleted.
    ///
    /// # Errors
    /// [`Error::NotFound`] when the proposal does not exist; [`Error::Storage`] otherwise.
    pub async fn add_revision(
        &self,
        proposal: ProposalId,
        edited_by: CitizenId,
        new: &NewRevision,
    ) -> Result<RevisionRow> {
        let now = self.clock.now();
        let mut tx = self.db.begin().await.map_err(map_sqlx)?;

        // Lock the proposal so the mirror-update and the append are consistent; absent => NotFound.
        if queries::lock_proposal(&mut *tx, proposal.as_uuid())
            .await
            .map_err(map_sqlx)?
            .is_none()
        {
            return Err(Error::NotFound("proposal not found".to_string()));
        }

        let revision = queries::insert_revision(
            &mut *tx,
            Uuid::now_v7(),
            proposal.as_uuid(),
            &new.title,
            &new.body,
            edited_by.as_uuid(),
            now,
        )
        .await
        .map_err(map_sqlx)?;
        queries::update_content(&mut *tx, proposal.as_uuid(), &new.title, &new.body)
            .await
            .map_err(map_sqlx)?;
        tx.commit().await.map_err(map_sqlx)?;
        Ok(revision)
    }

    /// List a proposal's revisions (oldest-first) with keyset pagination.
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn list_revisions(
        &self,
        proposal: ProposalId,
        after: Option<Uuid>,
        limit: i64,
    ) -> Result<Vec<RevisionRow>> {
        let limit = limit.clamp(1, 100);
        queries::list_revisions(&self.db, proposal.as_uuid(), after, limit)
            .await
            .map_err(map_sqlx)
    }

    /// Dispatch an inbound event to the matching handler. Unknown/irrelevant envelopes are an
    /// idempotent no-op ([`Consumed::Ignored`]). `consensus.cluster.formed` carries no proposal id,
    /// so it is recognised but not actionable here — the merge event does the linking.
    ///
    /// # Errors
    /// Propagates the handler's error.
    pub async fn consume(&self, envelope: &EventEnvelope) -> Result<Consumed> {
        if let Some((proposal, cluster)) = events::consensus_proposal_merged(envelope) {
            return self
                .link_to_cluster(envelope.id, envelope.org, proposal, cluster)
                .await;
        }
        if let Some(proposal) = events::moderation_cleared(envelope) {
            return self
                .mark_published(envelope.id, envelope.org, proposal)
                .await;
        }
        if let Some((proposal, support_count)) = events::vote_tally_updated(envelope) {
            return self
                .apply_tally(envelope.id, envelope.org, proposal, support_count)
                .await;
        }
        Ok(Consumed::Ignored)
    }

    /// Fold an aggregate support tally into the proposal's own `support_count` (monotonic), then fire
    /// the threshold crossing if the policy holds. This crate never reads the votes tables — the
    /// count arrives on the privacy-safe `votes.tally.updated` event. Idempotent: a replayed or stale
    /// tally cannot lower support or re-fire the crossing.
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn apply_tally(
        &self,
        event: EventId,
        org: OrgId,
        proposal: ProposalId,
        support_count: u64,
    ) -> Result<Consumed> {
        let mut tx = self.db.begin().await.map_err(map_sqlx)?;
        let Some(row) = queries::lock_proposal(&mut *tx, proposal.as_uuid())
            .await
            .map_err(map_sqlx)?
        else {
            // Unknown proposal (delivery for another crate's proposal) — idempotent no-op.
            tx.commit().await.map_err(map_sqlx)?;
            return Ok(Consumed::Ignored);
        };

        // Idempotent consumer (ADR-0007): claim this delivery in the effect tx; a duplicate
        // delivery of the same originating event is a no-op (the monotonic fold is a complement).
        if !dsoc_db::consumed::claim_consumed(&mut *tx, "proposals", event)
            .await
            .map_err(map_sqlx)?
        {
            tx.commit().await.map_err(map_sqlx)?;
            return Ok(Consumed::TallyUpdated {
                proposal,
                support_count: row.support_count,
                crossed: false,
            });
        }

        let incoming = i64::try_from(support_count).unwrap_or(i64::MAX);
        let merged = merge_support(row.support_count, incoming);
        if merged != row.support_count {
            queries::set_support_count(&mut *tx, proposal.as_uuid(), merged)
                .await
                .map_err(map_sqlx)?;
        }

        let crossed = self.try_cross(&mut tx, org, &row, merged).await?;
        tx.commit().await.map_err(map_sqlx)?;
        Ok(Consumed::TallyUpdated {
            proposal,
            support_count: merged,
            crossed,
        })
    }

    /// Link a proposal to a consensus cluster (status -> `clustered`) and, if support has already
    /// reached the threshold, fire the crossing in the same transaction. Idempotent: a duplicate
    /// link is a no-op (guarded by `cluster_id IS NULL`).
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn link_to_cluster(
        &self,
        event: EventId,
        org: OrgId,
        proposal: ProposalId,
        cluster: ClusterId,
    ) -> Result<Consumed> {
        let mut tx = self.db.begin().await.map_err(map_sqlx)?;
        let Some(row) = queries::lock_proposal(&mut *tx, proposal.as_uuid())
            .await
            .map_err(map_sqlx)?
        else {
            tx.commit().await.map_err(map_sqlx)?;
            return Ok(Consumed::Ignored);
        };

        // Idempotent consumer (ADR-0007): claim this delivery in the effect tx; a duplicate
        // delivery of the same originating event is a no-op (the `cluster_id IS NULL` guard
        // complements it for distinct events).
        if !dsoc_db::consumed::claim_consumed(&mut *tx, "proposals", event)
            .await
            .map_err(map_sqlx)?
        {
            let effective = row.cluster_id.unwrap_or_else(|| cluster.as_uuid());
            tx.commit().await.map_err(map_sqlx)?;
            return Ok(Consumed::Linked {
                proposal,
                cluster: ClusterId::from_uuid(effective),
                crossed: false,
            });
        }

        queries::link_cluster(&mut *tx, proposal.as_uuid(), cluster.as_uuid())
            .await
            .map_err(map_sqlx)?;

        // The effective cluster is the one it was already linked to (idempotent) or the new one.
        let effective = row.cluster_id.unwrap_or_else(|| cluster.as_uuid());
        let support = row.support_count;
        // Build a row view reflecting the just-applied link so the crossing check sees the cluster.
        let linked_row = ProposalRow {
            cluster_id: Some(effective),
            ..row
        };
        let crossed = self.try_cross(&mut tx, org, &linked_row, support).await?;
        tx.commit().await.map_err(map_sqlx)?;
        Ok(Consumed::Linked {
            proposal,
            cluster: ClusterId::from_uuid(effective),
            crossed,
        })
    }

    /// Publish a proposal (gated by `moderation.cleared`) and emit `proposals.published` through the
    /// outbox. Idempotent: a duplicate clearance does not re-publish or re-emit.
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn mark_published(
        &self,
        event: EventId,
        org: OrgId,
        proposal: ProposalId,
    ) -> Result<Consumed> {
        let now = self.clock.now();
        let mut tx = self.db.begin().await.map_err(map_sqlx)?;
        let Some(row) = queries::lock_proposal(&mut *tx, proposal.as_uuid())
            .await
            .map_err(map_sqlx)?
        else {
            tx.commit().await.map_err(map_sqlx)?;
            return Ok(Consumed::Ignored);
        };

        // Idempotent consumer (ADR-0007): claim this delivery in the effect tx; a duplicate
        // delivery of the same originating event does not re-publish (the `published_at IS NULL`
        // guard complements it for distinct events).
        if !dsoc_db::consumed::claim_consumed(&mut *tx, "proposals", event)
            .await
            .map_err(map_sqlx)?
        {
            tx.commit().await.map_err(map_sqlx)?;
            return Ok(Consumed::Published {
                proposal,
                emitted: false,
            });
        }

        let affected = queries::publish(&mut *tx, proposal.as_uuid(), now)
            .await
            .map_err(map_sqlx)?;
        let emitted = affected == 1;
        if emitted {
            let envelope = events::envelope(
                &self.clock,
                org,
                Event::ProposalPublished {
                    proposal,
                    mandate: MandateId::from_uuid(row.mandate_id),
                },
            );
            dsoc_db::outbox::publish_tx(&mut *tx, &envelope)
                .await
                .map_err(map_sqlx)?;
        }
        tx.commit().await.map_err(map_sqlx)?;
        Ok(Consumed::Published { proposal, emitted })
    }

    /// Apply the threshold-crossing policy under the held row lock. When it fires for the first time
    /// (`mark_threshold_crossed` affects one row), emit `proposals.threshold.crossed` carrying the
    /// cluster id through the outbox, atomically with the guard write. Returns whether it fired.
    async fn try_cross(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        org: OrgId,
        row: &ProposalRow,
        support_count: i64,
    ) -> Result<bool> {
        let Some(cluster_uuid) = row.cluster_id else {
            return Ok(false); // unclustered proposals cannot cross (the thesis guard)
        };
        let already = row.threshold_crossed_at.is_some();
        if !crossing_fires(true, support_count, row.threshold, already) {
            return Ok(false);
        }
        let now = self.clock.now();
        let affected = queries::mark_threshold_crossed(&mut **tx, row.id, now)
            .await
            .map_err(map_sqlx)?;
        if affected != 1 {
            // Lost the race to another transaction; the crossing already fired exactly once.
            return Ok(false);
        }
        // Multi-destinatário (0537): o sinal de consequência carrega TODOS os gabinetes,
        // pro consequence abrir um SLA por gabinete. Fallback pro principal se a lista
        // estiver vazia (proposta anterior ao backfill — não deve acontecer).
        let mut mandates: Vec<MandateId> = queries::target_mandate_ids(&mut **tx, row.id)
            .await
            .map_err(map_sqlx)?
            .into_iter()
            .map(MandateId::from_uuid)
            .collect();
        if mandates.is_empty() {
            mandates.push(MandateId::from_uuid(row.mandate_id));
        }
        let envelope = events::envelope(
            &self.clock,
            org,
            Event::ProposalThresholdCrossed {
                proposal: ProposalId::from_uuid(row.id),
                cluster: ClusterId::from_uuid(cluster_uuid),
                mandate: MandateId::from_uuid(row.mandate_id),
                mandates,
            },
        );
        dsoc_db::outbox::publish_tx(&mut **tx, &envelope)
            .await
            .map_err(map_sqlx)?;
        Ok(true)
    }
}

/// Map a `sqlx` failure onto the canonical error model (CONTRIBUTING.md wiring conventions):
/// missing row -> `NotFound`, unique/foreign-key violation -> `Conflict`, everything else ->
/// `Storage` (logged server-side, never surfaced raw).
fn map_sqlx(err: sqlx::Error) -> Error {
    match err {
        sqlx::Error::RowNotFound => Error::NotFound("proposal not found".to_string()),
        sqlx::Error::Database(ref db) if db.is_unique_violation() => {
            Error::Conflict("proposal already exists".to_string())
        }
        sqlx::Error::Database(ref db) if db.is_foreign_key_violation() => {
            Error::Conflict("referenced org or mandate does not exist".to_string())
        }
        other => Error::Storage(Box::new(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_sqlx_row_not_found_is_not_found() {
        assert_eq!(map_sqlx(sqlx::Error::RowNotFound).code(), "not_found");
    }

    #[test]
    fn map_sqlx_other_is_storage() {
        assert_eq!(map_sqlx(sqlx::Error::PoolTimedOut).code(), "storage_error");
    }
}

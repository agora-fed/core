//! The consequence service — the accountability engine. It holds the pool, the injected [`Clock`],
//! and the [`SlaPolicy`] window. It starts SLA clocks when a clustered proposal crosses its directed
//! threshold, records official responses, and sweeps expired clocks into permanent public silence.
//!
//! Every state change emits its catalog event through the **transactional outbox** (ADR-0006) inside
//! the same transaction as the write, so the domain change and the signal commit atomically — an
//! accountability platform must never silently drop an SLA-started or public-silence event. All
//! failures map onto the canonical [`dsoc_core::Error`].

use std::sync::Arc;

use chrono::{DateTime, Utc};
use dsoc_api_contract::SlaStatus;
use dsoc_core::events::Event;
use dsoc_core::ids::{MandateId, OrgId, SlaId};
use dsoc_core::{Clock, Error, Result};
use dsoc_db::Db;
use uuid::Uuid;

use crate::domain::{self, SlaInput, SlaPolicy};
use crate::events;
use crate::queries::{self, SlaRow};

/// Upper bound on SLAs processed per `sweep_expired` call. The sweep is idempotent and repeatable, so
/// a bounded batch keeps each pass cheap while a scheduler drives it to completion.
const SWEEP_BATCH: i64 = 256;

/// The result of starting an SLA: the stored row plus whether THIS call created it (`false` means a
/// duplicate threshold event found an existing clock — no second `started` event was emitted).
#[derive(Debug, Clone)]
pub struct StartedSla {
    /// The SLA row (the REAL stored row, even on the idempotent no-op path).
    pub sla: SlaRow,
    /// `true` if this call started the clock; `false` if it already existed.
    pub newly_started: bool,
}

/// The result of an official response: the SLA and its new terminal status.
#[derive(Debug, Clone, Copy)]
pub struct RespondOutcome {
    /// The SLA that was resolved.
    pub sla_id: SlaId,
    /// The new status (`Answered` or `Acted`).
    pub status: SlaStatus,
}

/// The consequence / accountability engine.
#[derive(Clone)]
pub struct ConsequenceService {
    db: Db,
    clock: Arc<dyn Clock>,
    policy: SlaPolicy,
}

impl std::fmt::Debug for ConsequenceService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConsequenceService")
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

impl ConsequenceService {
    /// Build a service with an explicit policy (used by tests to inject a short window).
    #[must_use]
    pub fn new(db: Db, clock: Arc<dyn Clock>, policy: SlaPolicy) -> Self {
        Self { db, clock, policy }
    }

    /// Build a service from the shared application state with the default SLA window.
    #[must_use]
    pub fn from_state(state: &dsoc_app::AppState) -> Self {
        Self::new(state.db.clone(), state.clock.clone(), SlaPolicy::default())
    }

    /// Start an SLA clock against an official for a clustered proposal that crossed its threshold.
    ///
    /// Idempotent per `(proposal, cluster, mandate)` — desde a 0538 o relógio é POR GABINETE — so a
    /// duplicate `proposals.threshold.crossed` (at-least-once delivery) cannot start a second clock
    /// against the same official. The first call inserts the row, emits
    /// `consequence.sla.started` carrying the due time through the outbox, and returns
    /// `newly_started = true`. A duplicate finds the existing row, emits nothing, and returns the REAL
    /// stored row with `newly_started = false`.
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn start_sla(
        &self,
        org: OrgId,
        threshold: events::ThresholdCrossed,
    ) -> Result<StartedSla> {
        let now = self.clock.now();
        let due = self.policy.due_at(now);

        let mut tx = self.db.begin().await.map_err(map_sqlx)?;

        let inserted = queries::insert_sla_idempotent(
            &mut *tx,
            SlaId::new().as_uuid(),
            org.as_uuid(),
            threshold.mandate.as_uuid(),
            threshold.cluster.as_uuid(),
            threshold.proposal.as_uuid(),
            now,
            due,
            now,
        )
        .await
        .map_err(map_sqlx)?;

        match inserted {
            Some(row) => {
                let envelope = events::envelope(
                    org,
                    now,
                    Event::ConsequenceSlaStarted {
                        sla: SlaId::from_uuid(row.id),
                        mandate: threshold.mandate,
                        cluster: threshold.cluster,
                        due,
                    },
                );
                dsoc_db::outbox::publish_tx(&mut *tx, &envelope)
                    .await
                    .map_err(map_sqlx)?;
                tx.commit().await.map_err(map_sqlx)?;
                Ok(StartedSla {
                    sla: row,
                    newly_started: true,
                })
            }
            None => {
                // Duplicate threshold event: re-select the real stored row, emit nothing.
                let row = queries::get_sla_by_proposal_cluster(
                    &mut *tx,
                    threshold.proposal.as_uuid(),
                    threshold.cluster.as_uuid(),
                    threshold.mandate.as_uuid(),
                )
                .await
                .map_err(map_sqlx)?;
                tx.commit().await.map_err(map_sqlx)?;
                Ok(StartedSla {
                    sla: row,
                    newly_started: false,
                })
            }
        }
    }

    /// Consume a `proposals.threshold.crossed` envelope, starting ONE SLA PER GABINETE
    /// (0537 multi-destinatário): o evento carrega todos os mandatos da proposta e cada um
    /// ganha seu próprio relógio, escada de avisos e registro de silêncio. Eventos antigos
    /// (sem a lista) caem pro mandato principal — comportamento pré-0537. Non-matching
    /// envelopes are ignored (idempotent no-op), returning an empty vec.
    ///
    /// # Errors
    /// Mirrors [`Self::start_sla`]; a falha em um gabinete interrompe e propaga (o redelivery
    /// at-least-once completa os restantes — cada início é idempotente por si).
    pub async fn consume(
        &self,
        envelope: &dsoc_core::events::EventEnvelope,
    ) -> Result<Vec<StartedSla>> {
        let Some(crossings) = events::threshold_crossed_all(envelope) else {
            return Ok(Vec::new());
        };
        let mut started = Vec::with_capacity(crossings.len());
        for threshold in crossings {
            started.push(self.start_sla(envelope.org, threshold).await?);
        }
        Ok(started)
    }

    /// Record an official's response to an SLA. The state machine decides the terminal status
    /// (`Answered`, or `Acted` when the response carries a concrete commitment). The SLA row is taken
    /// `FOR UPDATE` and the status update is guarded by the expected prior state, closing the TOCTOU
    /// race against a concurrent expiry sweep. A write-once `consequence_outcome` row makes the
    /// resolution permanent, and `consequence.official.responded` is emitted through the outbox.
    ///
    /// The caller (the gateway HTTP handler) is responsible for AUTHORIZING the official before this
    /// is invoked (Directory level) — this method trusts the SLA's own `mandate_id`, never a mandate
    /// taken from the request body.
    ///
    /// # Errors
    /// - [`Error::Validation`] when `body` is blank.
    /// - [`Error::NotFound`] when the SLA does not exist.
    /// - [`Error::Conflict`] when the SLA is already resolved (answered/acted/ignored). The existing
    ///   public outcome — including recorded silence — is left untouched.
    /// - [`Error::Storage`] on any other persistence failure.
    pub async fn respond(
        &self,
        sla_id: SlaId,
        body: &str,
        committed: bool,
    ) -> Result<RespondOutcome> {
        domain::validate_response_body(body)?;
        let now = self.clock.now();

        let mut tx = self.db.begin().await.map_err(map_sqlx)?;

        let row = queries::lock_sla(&mut *tx, sla_id.as_uuid())
            .await
            .map_err(map_sqlx)?;
        let current = domain::status_from_str(&row.status)?;
        // Conflict (Err) if the SLA is already terminal — the outcome is permanent.
        let target = domain::transition(current, SlaInput::Respond { committed }, now, row.due_at)?;
        let target_str = domain::status_as_str(target);

        let moved = queries::update_sla_status_guarded(&mut *tx, row.id, "pending", target_str)
            .await
            .map_err(map_sqlx)?;
        if !moved {
            // We hold the row lock, so this should not happen; treat a lost guard as a conflict.
            return Err(Error::Conflict(
                "sla already resolved; the public outcome is permanent".to_string(),
            ));
        }

        queries::insert_response(
            &mut *tx,
            Uuid::now_v7(),
            row.id,
            row.mandate_id,
            body,
            committed,
            now,
            now,
        )
        .await
        .map_err(map_sqlx)?;

        queries::insert_outcome_writeonce(&mut *tx, Uuid::now_v7(), row.id, target_str, now, now)
            .await
            .map_err(map_sqlx)?;

        let envelope = events::envelope(
            OrgId::from_uuid(row.org_id),
            now,
            Event::ConsequenceOfficialResponded {
                sla: SlaId::from_uuid(row.id),
                mandate: MandateId::from_uuid(row.mandate_id),
            },
        );
        dsoc_db::outbox::publish_tx(&mut *tx, &envelope)
            .await
            .map_err(map_sqlx)?;

        tx.commit().await.map_err(map_sqlx)?;
        Ok(RespondOutcome {
            sla_id: SlaId::from_uuid(row.id),
            status: target,
        })
    }

    /// Sweep an org's expired SLA clocks: every `pending` SLA in `org` whose `due_at <= now` is moved
    /// to `ignored`, a write-once `ignored` outcome is recorded (public silence is permanent), and
    /// `consequence.sla.expired` is emitted through the outbox. Returns how many SLAs expired.
    ///
    /// Scoped to `org` so a per-tenant sweep (the authorized `POST /consequence/sweep` endpoint, or a
    /// per-org scheduler pass) never touches another tenant's clocks.
    ///
    /// Deterministic and `sleep`-free: `now` is supplied by the caller from the injected clock
    /// (TESTING.md). Idempotent: it only touches `pending` rows, and the guarded update + write-once
    /// outcome make a re-run a no-op.
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn sweep_expired(&self, org: OrgId, now: DateTime<Utc>) -> Result<u64> {
        let mut tx = self.db.begin().await.map_err(map_sqlx)?;
        let candidates = queries::select_expired_pending(&mut *tx, org.as_uuid(), now, SWEEP_BATCH)
            .await
            .map_err(map_sqlx)?;

        let mut expired: u64 = 0;
        for candidate in &candidates {
            // The WHERE filter already guarantees Pending + (now >= due) → Ignored; re-evaluate the
            // pure transition so the decision lives in one place.
            let target =
                domain::transition(SlaStatus::Pending, SlaInput::Tick, now, candidate.due_at)?;
            if target != SlaStatus::Ignored {
                continue;
            }
            let moved =
                queries::update_sla_status_guarded(&mut *tx, candidate.id, "pending", "ignored")
                    .await
                    .map_err(map_sqlx)?;
            if !moved {
                continue;
            }
            queries::insert_outcome_writeonce(
                &mut *tx,
                Uuid::now_v7(),
                candidate.id,
                "ignored",
                now,
                now,
            )
            .await
            .map_err(map_sqlx)?;
            let envelope = events::envelope(
                OrgId::from_uuid(candidate.org_id),
                now,
                Event::ConsequenceSlaExpired {
                    sla: SlaId::from_uuid(candidate.id),
                    mandate: MandateId::from_uuid(candidate.mandate_id),
                },
            );
            dsoc_db::outbox::publish_tx(&mut *tx, &envelope)
                .await
                .map_err(map_sqlx)?;
            expired += 1;
        }

        tx.commit().await.map_err(map_sqlx)?;
        Ok(expired)
    }

    /// Sweep every tenant that currently has an expired-but-pending SLA clock. Enumerates the due
    /// orgs and runs the per-org [`Self::sweep_expired`] for each at the supplied `now`, returning the
    /// total number of SLAs moved into permanent public silence. This is the entry point the
    /// background scheduler calls on its interval (the per-org method stays the authorized HTTP path).
    ///
    /// Deterministic and `sleep`-free: `now` is the injected clock's time. Idempotent: a re-run after
    /// everything due has expired enumerates no orgs and returns `0`.
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn sweep_all_due(&self, now: DateTime<Utc>) -> Result<u64> {
        let orgs = queries::select_due_org_ids(&self.db, now)
            .await
            .map_err(map_sqlx)?;
        let mut total = 0u64;
        for org in orgs {
            total += self.sweep_expired(OrgId::from_uuid(org), now).await?;
        }
        Ok(total)
    }

    /// Fetch a single SLA by id.
    ///
    /// # Errors
    /// [`Error::NotFound`] when absent; [`Error::Storage`] otherwise.
    pub async fn get_sla(&self, id: SlaId) -> Result<SlaRow> {
        queries::get_sla(&self.db, id.as_uuid())
            .await
            .map_err(map_sqlx)
    }

    /// Read the recorded terminal outcome text for an SLA, if any.
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn outcome(&self, id: SlaId) -> Result<Option<String>> {
        queries::get_outcome(&self.db, id.as_uuid())
            .await
            .map_err(map_sqlx)
    }

    /// List an org's SLAs with keyset pagination, returning the rows and the total count.
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn list_slas(
        &self,
        org: OrgId,
        after: Option<SlaId>,
        limit: i64,
    ) -> Result<(Vec<SlaRow>, i64)> {
        let limit = limit.clamp(1, 100);
        let rows = queries::list_slas(&self.db, org.as_uuid(), after.map(|s| s.as_uuid()), limit)
            .await
            .map_err(map_sqlx)?;
        let total = queries::count_slas(&self.db, org.as_uuid())
            .await
            .map_err(map_sqlx)?;
        Ok((rows, total))
    }
}

/// Map a `sqlx` failure onto the canonical error model (CONTRIBUTING.md wiring conventions):
/// missing row -> `NotFound`, unique violation -> `Conflict`, everything else -> `Storage`.
fn map_sqlx(err: sqlx::Error) -> Error {
    match err {
        sqlx::Error::RowNotFound => Error::NotFound("sla not found".to_string()),
        sqlx::Error::Database(ref db) if db.is_unique_violation() => {
            Error::Conflict("sla already resolved".to_string())
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

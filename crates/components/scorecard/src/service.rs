//! The scorecard service: the pure projection plus the authorized promise surface.
//!
//! The scorecard owns **no business decisions** (PLAN.md). The projection methods rebuild a
//! politician's public record from the consequence/mandate event stream: each one appends to the
//! event-sourced ledger, recomputes the public counters from that ledger (so the denormalised
//! `answered`/`ignored` can never drift), and emits `scorecard.updated` through the **transactional
//! outbox** (ADR-0006) — the ledger append and the event row commit in the SAME transaction, so no
//! signal can be lost or duplicated relative to the change. Replays are idempotent: the ledger
//! dedupes by `sla_id` (delivery is at-least-once).
//!
//! The promise surface is the one citizen-facing mutation. It AUTHORIZES the authenticated caller
//! against the resource's REAL org via the injected [`Authorization`] port before any write, and the
//! delivery transition is guarded by the expected prior state to close the TOCTOU race.

use std::sync::Arc;

use dsoc_core::ids::{CitizenId, MandateId, OrgId, ScorecardId, SlaId};
use dsoc_core::{Authorization, Clock, Error, Result, VerificationLevel};
use dsoc_db::Db;
use uuid::Uuid;

use crate::domain::{self, Outcome, Promise, ScorecardView};
use crate::events;
use crate::queries::{self, Cursor};

/// Upper bound on free-text fields (promise text).
const MAX_TEXT_LEN: usize = 2_000;
/// Safety cap on a single scorecard's ledger read (per-mandate SLAs stay modest).
const MAX_LEDGER_READ: i64 = 100_000;
/// Default page size for list endpoints.
pub const DEFAULT_PAGE_LIMIT: i64 = 50;
/// Hard cap on page size to keep reads bounded.
pub const MAX_PAGE_LIMIT: i64 = 200;
/// Minimum verification an official needs to record/deliver a public promise. Officials are
/// directory-verified (PLAN.md `mandates`: invited officials reach `Directory`).
const MIN_OFFICIAL_LEVEL: VerificationLevel = VerificationLevel::Directory;

/// The scorecard projection + promise service. Cheap to construct per request from `AppState`.
pub struct ScorecardService {
    db: Db,
    clock: Arc<dyn Clock>,
    authz: Arc<dyn Authorization>,
}

impl std::fmt::Debug for ScorecardService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScorecardService").finish_non_exhaustive()
    }
}

impl ScorecardService {
    /// Build a service from its injected ports.
    #[must_use]
    pub fn new(db: Db, clock: Arc<dyn Clock>, authz: Arc<dyn Authorization>) -> Self {
        Self { db, clock, authz }
    }

    /// Build a service from the shared application state.
    #[must_use]
    pub fn from_state(state: &dsoc_app::AppState) -> Self {
        Self::new(state.db.clone(), state.clock.clone(), state.authz.clone())
    }

    // --- projection (event-driven; no caller to authorize — the dispatcher is trusted) ---------

    /// Consume `mandates.official.onboarded`: get-or-create a zeroed scorecard for the mandate and
    /// emit `scorecard.updated`. Idempotent — re-onboarding returns the existing scorecard with its
    /// counters intact (the upsert never resets `answered`/`ignored`).
    ///
    /// # Errors
    /// [`Error::Storage`] (or a mapped variant) on persistence/publish failure.
    pub async fn on_onboarded(&self, org: OrgId, mandate: MandateId) -> Result<ScorecardView> {
        let now = self.clock.now();
        let mut tx = self.db.begin().await.map_err(map_sqlx)?;

        let scorecard_id = queries::upsert_scorecard(
            &mut *tx,
            ScorecardId::new().as_uuid(),
            org.as_uuid(),
            mandate.as_uuid(),
            now,
        )
        .await
        .map_err(map_storage)?;

        let envelope = events::envelope(
            &self.clock,
            org,
            events::scorecard_updated(ScorecardId::from_uuid(scorecard_id), mandate),
        );
        dsoc_db::outbox::publish_tx(&mut *tx, &envelope)
            .await
            .map_err(map_sqlx)?;
        tx.commit().await.map_err(map_sqlx)?;

        self.view_for(scorecard_id).await
    }

    /// Consume `consequence.sla.expired`: append an ignored entry and recount. Emits
    /// `scorecard.updated` only when a NEW entry was appended; a replay of the same `sla_id` is a
    /// no-op (no double-count, no duplicate signal).
    ///
    /// # Errors
    /// [`Error::Storage`] (or a mapped variant) on persistence/publish failure.
    pub async fn on_sla_expired(
        &self,
        org: OrgId,
        mandate: MandateId,
        sla: SlaId,
        occurred_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<ScorecardView> {
        self.append_outcome(org, mandate, sla, Outcome::Ignored, None, occurred_at)
            .await
    }

    /// Consume `consequence.official.responded`: append an answered entry recording its latency and
    /// recount. Emits `scorecard.updated` only on a new entry; replays are idempotent.
    ///
    /// # Errors
    /// [`Error::Validation`] for a negative/non-finite latency; [`Error::Storage`] (or a mapped
    /// variant) on persistence/publish failure.
    pub async fn on_responded(
        &self,
        org: OrgId,
        mandate: MandateId,
        sla: SlaId,
        response_hours: f64,
        occurred_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<ScorecardView> {
        let hours = domain::validate_response_hours(response_hours).map_err(validation)?;
        self.append_outcome(
            org,
            mandate,
            sla,
            Outcome::Answered,
            Some(hours),
            occurred_at,
        )
        .await
    }

    /// Shared append path for both SLA outcomes: ensure the scorecard exists, append the ledger
    /// entry idempotently, and — only when the entry is new — recount the counters from the ledger
    /// and emit `scorecard.updated`, all inside ONE transaction (ADR-0006).
    async fn append_outcome(
        &self,
        org: OrgId,
        mandate: MandateId,
        sla: SlaId,
        outcome: Outcome,
        response_hours: Option<f64>,
        occurred_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<ScorecardView> {
        let now = self.clock.now();
        let mut tx = self.db.begin().await.map_err(map_sqlx)?;

        // Ensure the scorecard exists even if the outcome arrives before onboarding (out-of-order,
        // at-least-once delivery): the projection must be resilient, not assume an ordering.
        let scorecard_id = queries::upsert_scorecard(
            &mut *tx,
            ScorecardId::new().as_uuid(),
            org.as_uuid(),
            mandate.as_uuid(),
            now,
        )
        .await
        .map_err(map_storage)?;

        let inserted = queries::insert_entry(
            &mut *tx,
            Uuid::now_v7(),
            scorecard_id,
            sla.as_uuid(),
            outcome,
            response_hours,
            occurred_at,
            now,
        )
        .await
        .map_err(map_storage)?;

        if inserted.is_none() {
            // Replay of an already-recorded SLA: nothing changes. Commit the (no-op) ensure and
            // return the unchanged projection — no recount, no duplicate `scorecard.updated`.
            tx.commit().await.map_err(map_sqlx)?;
            return self.view_for(scorecard_id).await;
        }

        // Recompute the public counters straight from the ledger so they always equal a recount.
        let (answered, ignored) = queries::recount_entries(&mut *tx, scorecard_id)
            .await
            .map_err(map_storage)?;
        queries::update_counters(&mut *tx, scorecard_id, answered, ignored, now)
            .await
            .map_err(map_storage)?;

        let envelope = events::envelope(
            &self.clock,
            org,
            events::scorecard_updated(ScorecardId::from_uuid(scorecard_id), mandate),
        );
        dsoc_db::outbox::publish_tx(&mut *tx, &envelope)
            .await
            .map_err(map_sqlx)?;
        tx.commit().await.map_err(map_sqlx)?;

        self.view_for(scorecard_id).await
    }

    // --- public reads ------------------------------------------------------------------------

    /// Fetch a politician's public scorecard by mandate.
    ///
    /// # Errors
    /// [`Error::NotFound`] when the mandate has no scorecard; [`Error::Storage`] otherwise.
    pub async fn get_by_mandate(&self, mandate: MandateId) -> Result<ScorecardView> {
        let card = queries::get_scorecard_by_mandate(&self.db, mandate.as_uuid())
            .await
            .map_err(map_storage)?
            .ok_or_else(|| Error::NotFound("scorecard not found for mandate".to_owned()))?;
        let hours = queries::answered_response_hours(&self.db, card.id)
            .await
            .map_err(map_storage)?;
        Ok(ScorecardView::from_parts(&card, &hours))
    }

    /// List an org's scorecards newest-first (keyset-paginated), each with its derived median.
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn list(
        &self,
        org: OrgId,
        cursor: Option<Cursor>,
        limit: i64,
    ) -> Result<Vec<ScorecardView>> {
        let cards = queries::list_scorecards(&self.db, org.as_uuid(), cursor, clamp_limit(limit))
            .await
            .map_err(map_storage)?;
        let mut views = Vec::with_capacity(cards.len());
        for card in cards {
            let hours = queries::answered_response_hours(&self.db, card.id)
                .await
                .map_err(map_storage)?;
            views.push(ScorecardView::from_parts(&card, &hours));
        }
        Ok(views)
    }

    /// List a mandate's public promises newest-first (keyset-paginated).
    ///
    /// # Errors
    /// [`Error::NotFound`] when the mandate has no scorecard; [`Error::Storage`] otherwise.
    pub async fn list_promises(
        &self,
        mandate: MandateId,
        cursor: Option<Cursor>,
        limit: i64,
    ) -> Result<Vec<Promise>> {
        let card = queries::get_scorecard_by_mandate(&self.db, mandate.as_uuid())
            .await
            .map_err(map_storage)?
            .ok_or_else(|| Error::NotFound("scorecard not found for mandate".to_owned()))?;
        queries::list_promises(&self.db, card.id, cursor, clamp_limit(limit))
            .await
            .map_err(map_storage)
    }

    // --- authorized mutations ----------------------------------------------------------------

    /// Record a public promise for a mandate. AUTHORIZES the authenticated `actor` against the
    /// scorecard's REAL org (never an org taken from the request body) at directory level before
    /// writing.
    ///
    /// # Errors
    /// [`Error::Unauthorized`]/[`Error::Forbidden`] when the actor is insufficiently verified;
    /// [`Error::Validation`] for empty/oversized text; [`Error::NotFound`] when the mandate has no
    /// scorecard; [`Error::Storage`] otherwise.
    pub async fn record_promise(
        &self,
        actor: CitizenId,
        mandate: MandateId,
        text: &str,
    ) -> Result<Promise> {
        let card = queries::get_scorecard_by_mandate(&self.db, mandate.as_uuid())
            .await
            .map_err(map_storage)?
            .ok_or_else(|| Error::NotFound("scorecard not found for mandate".to_owned()))?;
        // Authorize against the resource's actual org, derived from storage — not the caller's input.
        self.authz
            .require(OrgId::from_uuid(card.org_id), actor, MIN_OFFICIAL_LEVEL)
            .await?;

        let text = domain::validate_nonempty("text", text, MAX_TEXT_LEN).map_err(validation)?;
        let now = self.clock.now();
        let promise = Promise {
            id: Uuid::now_v7(),
            scorecard_id: card.id,
            text,
            made_at: now,
            delivered: false,
            delivered_at: None,
            created_at: now,
        };
        queries::insert_promise(&self.db, &promise)
            .await
            .map_err(map_storage)?;
        Ok(promise)
    }

    /// Mark a promise delivered. AUTHORIZES the authenticated `actor` against the promise's REAL org
    /// at directory level, then flips `delivered` false -> true under an optimistic guard. Idempotent:
    /// a promise already delivered returns its current state (0 rows affected is the already-in-target
    /// case), never an error.
    ///
    /// # Errors
    /// [`Error::NotFound`] when the promise does not exist; [`Error::Unauthorized`]/
    /// [`Error::Forbidden`] when the actor is insufficiently verified; [`Error::Storage`] otherwise.
    pub async fn mark_promise_delivered(
        &self,
        actor: CitizenId,
        promise_id: Uuid,
    ) -> Result<Promise> {
        let promise = queries::get_promise(&self.db, promise_id)
            .await
            .map_err(map_storage)?;
        let card = queries::get_scorecard(&self.db, promise.scorecard_id)
            .await
            .map_err(map_storage)?;
        self.authz
            .require(OrgId::from_uuid(card.org_id), actor, MIN_OFFICIAL_LEVEL)
            .await?;

        let now = self.clock.now();
        match queries::mark_promise_delivered(&self.db, promise_id, now)
            .await
            .map_err(map_storage)?
        {
            Some(updated) => Ok(updated),
            // Already delivered: re-read the current row so the response reflects committed state.
            None => queries::get_promise(&self.db, promise_id)
                .await
                .map_err(map_storage),
        }
    }

    // --- helpers -----------------------------------------------------------------------------

    /// Read back the public view (counters + derived median) for a scorecard id.
    async fn view_for(&self, scorecard_id: Uuid) -> Result<ScorecardView> {
        let card = queries::get_scorecard(&self.db, scorecard_id)
            .await
            .map_err(map_storage)?;
        let hours = queries::answered_response_hours(&self.db, card.id)
            .await
            .map_err(map_storage)?;
        Ok(ScorecardView::from_parts(&card, &hours))
    }
}

/// Maximum number of ledger rows a single audit read returns (exposed for the audit query bound).
#[must_use]
pub const fn max_ledger_read() -> i64 {
    MAX_LEDGER_READ
}

fn clamp_limit(limit: i64) -> i64 {
    limit.clamp(1, MAX_PAGE_LIMIT)
}

fn validation(err: domain::ParseError) -> Error {
    Error::Validation(err.to_string())
}

/// Map a raw `sqlx` failure (from transaction/outbox calls) onto the canonical error model.
fn map_sqlx(err: sqlx::Error) -> Error {
    match err {
        sqlx::Error::RowNotFound => Error::NotFound("scorecard entity not found".to_owned()),
        sqlx::Error::Database(ref db) if db.is_unique_violation() => {
            Error::Conflict("scorecard entity already exists".to_owned())
        }
        other => Error::Storage(Box::new(other)),
    }
}

/// Map a storage-layer [`queries::Error`] onto the canonical [`dsoc_core::Error`]
/// (`RowNotFound` -> `NotFound`; unique -> `Conflict`; FK/check/not-null -> `Validation`; corrupt
/// stored value -> `Storage`; everything else -> `Storage`).
fn map_storage(err: queries::Error) -> Error {
    match err {
        queries::Error::Db(sqlx::Error::RowNotFound) => {
            Error::NotFound("scorecard entity not found".to_owned())
        }
        queries::Error::Db(sqlx::Error::Database(db_err)) => match db_err.code().as_deref() {
            Some("23505") => Error::Conflict("scorecard entity already exists".to_owned()),
            Some("23503" | "23514" | "23502") => {
                Error::Validation("scorecard input violates a constraint".to_owned())
            }
            _ => Error::Storage(Box::new(db_err)),
        },
        queries::Error::Db(other) => Error::Storage(Box::new(other)),
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
    fn row_not_found_maps_to_not_found() {
        let mapped = map_storage(queries::Error::Db(sqlx::Error::RowNotFound));
        assert!(matches!(mapped, Error::NotFound(_)));
        assert_eq!(mapped.code(), "not_found");
    }

    #[test]
    fn sqlx_row_not_found_maps_to_not_found() {
        assert_eq!(map_sqlx(sqlx::Error::RowNotFound).code(), "not_found");
        assert_eq!(map_sqlx(sqlx::Error::PoolTimedOut).code(), "storage_error");
    }

    #[test]
    fn decode_error_maps_to_storage() {
        let parse = domain::ParseError {
            field: "outcome",
            value: "garbage".to_owned(),
        };
        let mapped = map_storage(queries::Error::Decode(parse));
        assert_eq!(mapped.code(), "storage_error");
    }

    #[test]
    fn validation_helper_maps_parse_error() {
        let parse = domain::ParseError {
            field: "text",
            value: String::new(),
        };
        assert!(matches!(validation(parse), Error::Validation(_)));
    }

    #[test]
    fn official_level_is_directory() {
        assert_eq!(MIN_OFFICIAL_LEVEL, VerificationLevel::Directory);
        assert_eq!(max_ledger_read(), MAX_LEDGER_READ);
    }
}

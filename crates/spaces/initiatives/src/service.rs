//! The initiatives service: orchestrates authorization, the injected clock, and the `sqlx`
//! queries, mapping every failure onto the canonical [`dsoc_core::Error`].
//!
//! It holds the `Db` pool, an `Arc<dyn Clock>` (time is injected, never ambient), an
//! `Arc<dyn EventBus>` (the publish port — initiatives currently emits no cross-crate events: the
//! frozen `dsoc_core::events::Event` catalog has no `initiatives.*` variants, and adding one is a
//! core change), and an `Arc<dyn Authorization>` used to gate every mutation.
//!
//! Signing is the heart of the domain: a citizen's signature is inserted idempotently (the
//! `(initiative, citizen)` unique key means a repeat sign is a no-op), the running tally is
//! incremented in the SAME transaction, and crossing the threshold sets `reached_at` exactly once
//! via a `reached_at IS NULL` guard (one-shot escalation).

use std::sync::Arc;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use dsoc_app::AppState;
use dsoc_core::ids::{CitizenId, OrgId};
use dsoc_core::{Authorization, Clock, Error, EventBus, Result};
use dsoc_db::Db;

use crate::domain::{self, MIN_MUTATION_LEVEL};
use crate::queries::{self, InitiativeRow};

/// Default page size for list endpoints when the caller does not specify one.
pub const DEFAULT_PAGE_LIMIT: u32 = 50;

/// Hard cap on a list page, applied even if the caller requests more (unbounded reads must be
/// bounded — PLAN.md).
pub const MAX_PAGE_LIMIT: u32 = 100;

/// A citizen initiative as the service sees it.
#[derive(Debug, Clone)]
pub struct Initiative {
    /// Real stored initiative id (RETURNING from the insert; never a phantom id).
    pub id: Uuid,
    /// The organization hosting the initiative.
    pub org: OrgId,
    /// Human title.
    pub title: String,
    /// Full body text.
    pub body: String,
    /// Signatures required to escalate.
    pub threshold: i32,
    /// Current running signature tally.
    pub signature_count: i64,
    /// When the initiative first reached its threshold (one-shot escalation marker), if it has.
    pub reached_at: Option<DateTime<Utc>>,
    /// When the initiative was created (injected clock).
    pub created_at: DateTime<Utc>,
}

impl Initiative {
    /// Whether this initiative has crossed its signature threshold (escalated).
    #[must_use]
    pub fn has_reached(&self) -> bool {
        self.reached_at.is_some()
    }
}

/// The outcome of signing an initiative.
#[derive(Debug, Clone)]
pub struct SignOutcome {
    /// The real stored signature id (newly inserted, or the existing one on a repeat sign).
    pub signature_id: Uuid,
    /// The initiative signed.
    pub initiative_id: Uuid,
    /// The citizen who signed.
    pub citizen: CitizenId,
    /// The signature tally after this sign.
    pub signature_count: i64,
    /// The threshold required to escalate.
    pub threshold: i32,
    /// When the initiative reached its threshold (set, possibly by this very sign), if it has.
    pub reached_at: Option<DateTime<Utc>>,
    /// Whether THIS call recorded a new signature (`false` on an idempotent repeat sign).
    pub newly_signed: bool,
}

impl SignOutcome {
    /// Whether the initiative has reached its threshold after this sign.
    #[must_use]
    pub fn has_reached(&self) -> bool {
        self.reached_at.is_some()
    }
}

/// The citizen-initiatives service.
#[derive(Clone)]
pub struct InitiativeService {
    db: Db,
    clock: Arc<dyn Clock>,
    /// Injected event bus, retained for future fire-and-forget emission. Initiatives emits no
    /// cross-crate events today (the frozen catalog has no `initiatives.*` variants); the port is
    /// held so a future ADR can wire emission without changing construction.
    bus: Arc<dyn EventBus>,
    authz: Arc<dyn Authorization>,
}

impl std::fmt::Debug for InitiativeService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InitiativeService")
            .field("db", &"PgPool")
            .finish_non_exhaustive()
    }
}

impl InitiativeService {
    /// Construct from explicitly injected ports (used directly by integration tests).
    #[must_use]
    pub fn new(
        db: Db,
        clock: Arc<dyn Clock>,
        bus: Arc<dyn EventBus>,
        authz: Arc<dyn Authorization>,
    ) -> Self {
        Self {
            db,
            clock,
            bus,
            authz,
        }
    }

    /// Construct from the shared [`AppState`] the gateway injects (ADR-0004 wiring).
    #[must_use]
    pub fn from_state(state: &AppState) -> Self {
        Self {
            db: state.db.clone(),
            clock: Arc::clone(&state.clock),
            bus: Arc::clone(&state.bus),
            authz: Arc::clone(&state.authz),
        }
    }

    /// The injected event-bus publish port. Initiatives persists state but emits no cross-crate
    /// events for now; this accessor keeps the wired port observable (and future-proof).
    #[must_use]
    pub fn event_bus(&self) -> &Arc<dyn EventBus> {
        &self.bus
    }

    /// Assert the caller may perform an initiative mutation in `org` (never trusts a body-supplied
    /// id; it checks the AUTHENTICATED caller against the injected [`Authorization`]).
    async fn authorize_mutation(&self, org: OrgId, caller: CitizenId) -> Result<()> {
        self.authz.require(org, caller, MIN_MUTATION_LEVEL).await
    }

    /// Create a citizen initiative. Validates the title, body, and threshold at the boundary, then
    /// persists it with a zero tally and no escalation marker.
    ///
    /// # Errors
    /// [`Error::Forbidden`] if the caller is unauthorized; [`Error::Validation`] on a malformed
    /// title/body/threshold; [`Error::Storage`] on persistence failure.
    pub async fn create(
        &self,
        org: OrgId,
        caller: CitizenId,
        title: &str,
        body: &str,
        threshold: i32,
    ) -> Result<Initiative> {
        self.authorize_mutation(org, caller).await?;
        let title = domain::validate_title(title)?;
        let body = domain::validate_body(body)?;
        let threshold = domain::validate_threshold(threshold)?;
        let now = self.clock.now();

        let row = queries::insert_initiative(
            &self.db,
            Uuid::now_v7(),
            org.as_uuid(),
            &title,
            &body,
            threshold,
            now,
        )
        .await
        .map_err(map_sqlx)?;
        Ok(initiative_from_row(row))
    }

    /// Sign an initiative as the authenticated caller. Idempotent: a citizen who has already signed
    /// gets the same outcome without double-counting. A new signature increments the tally and, if
    /// it crosses the threshold, sets `reached_at` exactly once — all in one transaction.
    ///
    /// # Errors
    /// [`Error::Forbidden`] if unauthorized; [`Error::NotFound`] if the initiative is absent in
    /// `org`; [`Error::Storage`] on persistence failure.
    pub async fn sign(
        &self,
        org: OrgId,
        caller: CitizenId,
        initiative_id: Uuid,
    ) -> Result<SignOutcome> {
        self.authorize_mutation(org, caller).await?;
        let now = self.clock.now();

        let mut tx = self.db.begin().await.map_err(map_sqlx)?;

        // The initiative must exist within the caller's org (NotFound otherwise — no state change).
        let init = queries::get_initiative(&mut *tx, org.as_uuid(), initiative_id)
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| Error::NotFound("initiative not found".to_owned()))?;

        // Insert the signature idempotently. A new row returns its real id; a repeat sign yields
        // None (the citizen already signed), so we re-select the existing signature's real id.
        let inserted = queries::insert_signature_if_absent(
            &mut *tx,
            Uuid::now_v7(),
            initiative_id,
            caller.as_uuid(),
            now,
        )
        .await
        .map_err(map_sqlx)?;

        let outcome = match inserted {
            Some(signature_id) => {
                // New signature: increment the tally and, atomically, mark escalation if crossing.
                let counted = queries::increment_and_maybe_reach(&mut *tx, initiative_id, now)
                    .await
                    .map_err(map_sqlx)?;
                SignOutcome {
                    signature_id,
                    initiative_id,
                    citizen: caller,
                    signature_count: counted.signature_count,
                    threshold: counted.threshold,
                    reached_at: counted.reached_at,
                    newly_signed: true,
                }
            }
            None => {
                // Already signed: no increment. Return the existing signature and unchanged tally.
                let signature_id =
                    queries::get_signature_id(&mut *tx, initiative_id, caller.as_uuid())
                        .await
                        .map_err(map_sqlx)?;
                SignOutcome {
                    signature_id,
                    initiative_id,
                    citizen: caller,
                    signature_count: init.signature_count,
                    threshold: init.threshold,
                    reached_at: init.reached_at,
                    newly_signed: false,
                }
            }
        };

        tx.commit().await.map_err(map_sqlx)?;
        Ok(outcome)
    }

    /// Fetch a single initiative (read-only; reads are unrestricted).
    ///
    /// # Errors
    /// [`Error::NotFound`] if absent; [`Error::Storage`] on persistence failure.
    pub async fn get(&self, org: OrgId, initiative_id: Uuid) -> Result<Initiative> {
        let row = queries::get_initiative(&self.db, org.as_uuid(), initiative_id)
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| Error::NotFound("initiative not found".to_owned()))?;
        Ok(initiative_from_row(row))
    }

    /// List an org's initiatives with keyset pagination over id (ascending).
    ///
    /// # Errors
    /// [`Error::Storage`] on persistence failure.
    pub async fn list(
        &self,
        org: OrgId,
        after: Option<Uuid>,
        limit: Option<u32>,
    ) -> Result<Vec<Initiative>> {
        let rows = queries::list_initiatives(
            &self.db,
            org.as_uuid(),
            after,
            domain::clamp_limit(limit, MAX_PAGE_LIMIT),
        )
        .await
        .map_err(map_sqlx)?;
        Ok(rows.into_iter().map(initiative_from_row).collect())
    }
}

/// Map a raw `sqlx` failure onto the canonical, public-safe error model.
fn map_sqlx(error: sqlx::Error) -> Error {
    match error {
        sqlx::Error::RowNotFound => Error::NotFound("initiative not found".to_owned()),
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            Error::Conflict("initiative record already exists".to_owned())
        }
        other => {
            tracing::error!(error = %other, "initiatives storage failure");
            Error::Storage(Box::new(other))
        }
    }
}

/// Map an initiative row to its public domain type.
fn initiative_from_row(row: InitiativeRow) -> Initiative {
    Initiative {
        id: row.id,
        org: OrgId::from_uuid(row.org_id),
        title: row.title,
        body: row.body,
        threshold: row.threshold,
        signature_count: row.signature_count,
        reached_at: row.reached_at,
        created_at: row.created_at,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

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
    fn initiative_has_reached_reflects_marker() {
        let mut init = Initiative {
            id: Uuid::now_v7(),
            org: OrgId::new(),
            title: "t".to_owned(),
            body: "b".to_owned(),
            threshold: 3,
            signature_count: 3,
            reached_at: None,
            created_at: Utc::now(),
        };
        assert!(!init.has_reached());
        init.reached_at = Some(Utc::now());
        assert!(init.has_reached());
    }
}

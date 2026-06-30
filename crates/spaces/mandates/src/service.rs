//! `MandateRegistry` — the concrete mandate-lifecycle service. Holds the `Db` pool plus the
//! injected `Arc<dyn Clock>`, `Arc<dyn EventBus>`, and `Arc<dyn Authorization>` (ADR-0004 wiring).
//! It drives mandatory onboarding of public officials/candidates: invite → accept → onboarded,
//! identity bindings, and term-bound offices.
//!
//! Every mutation authorizes the caller via the injected [`Authorization`], emits its event through
//! the **transactional outbox** (ADR-0006) inside the same transaction as the write, and guards
//! state transitions with the expected prior state to close TOCTOU races. All `sqlx` failures are
//! mapped onto the canonical [`dsoc_core::Error`] model here.

use std::sync::Arc;

use chrono::{DateTime, NaiveDate, Utc};
use uuid::Uuid;

use dsoc_app::AppState;
use dsoc_core::events::EventEnvelope;
use dsoc_core::ids::{CitizenId, MandateId, OrgId};
use dsoc_core::{Authorization, Clock, Error, EventBus, Result, VerificationLevel};
use dsoc_db::Db;

use crate::domain::{
    self, OnboardingStatus, INVITATION_TTL_HOURS, MIN_MUTATION_LEVEL, ONBOARDING_BINDING_LEVEL,
};
use crate::events;
use crate::queries::{self, IdentityBindingRow, OfficeRow};

/// Default page size for list endpoints when the caller does not specify one.
pub const DEFAULT_PAGE_LIMIT: u32 = 50;

/// Hard cap on a list page, applied even if the caller requests more (unbounded reads must be
/// bounded — PLAN.md).
pub const MAX_PAGE_LIMIT: u32 = 100;

/// A persisted invitation, returned once to the inviter. `token` is the plaintext invite token —
/// it exists only in this value (the platform stores only its hash) and must be delivered to the
/// official's public email, then discarded.
#[derive(Debug, Clone)]
pub struct Invitation {
    /// Real stored invitation id (RETURNING from the insert; never a phantom id).
    pub id: Uuid,
    /// The mandate invited.
    pub mandate: MandateId,
    /// The public email the invite was addressed to.
    pub public_email: String,
    /// The one-time plaintext token to deliver (never persisted; only its hash is stored).
    pub token: String,
    /// When the invite was sent (injected clock).
    pub sent_at: DateTime<Utc>,
}

/// The result of a successful onboarding (invite acceptance).
#[derive(Debug, Clone)]
pub struct Onboarding {
    /// The mandate now onboarded.
    pub mandate: MandateId,
    /// When onboarding completed (injected clock).
    pub onboarded_at: DateTime<Utc>,
}

/// A mandate as the registry sees it: identity columns plus the DERIVED onboarding status and the
/// stable public ActivityPub-readiness handle (ADR-0005).
#[derive(Debug, Clone)]
pub struct MandateView {
    /// The mandate id.
    pub id: MandateId,
    /// Office held or sought.
    pub office: String,
    /// Public display name.
    pub display_name: String,
    /// Whether this is a candidacy (vs sitting office).
    pub is_candidate: bool,
    /// Derived onboarding status (never a stored enum).
    pub status: OnboardingStatus,
    /// Stable public handle (ADR-0005 federation seam).
    pub public_handle: String,
    /// Sigla do partido (when this is a real seated parliamentarian).
    pub party: Option<String>,
    /// Sigla da UF.
    pub uf: Option<String>,
    /// Casa: `camara` | `senado`.
    pub house: Option<String>,
    /// Object key in MinIO under bucket `dsoc-media`. Renders as `<MEDIA_BASE_URL>/<key>`.
    pub avatar_object_key: Option<String>,
}

/// A term-bound office record.
#[derive(Debug, Clone)]
pub struct Office {
    /// Office record id.
    pub id: Uuid,
    /// The mandate this office belongs to.
    pub mandate: MandateId,
    /// Office held or sought.
    pub office: String,
    /// Electoral district / municipality, if any.
    pub district: Option<String>,
    /// Term start date, if known.
    pub term_start: Option<NaiveDate>,
    /// Term end date, if known.
    pub term_end: Option<NaiveDate>,
    /// When the record was created.
    pub created_at: DateTime<Utc>,
}

/// The descriptive fields of a term-bound office, bundled so callers pass a single value (keeps
/// [`MandateRegistry::add_office`] within a sane argument count).
#[derive(Debug, Clone, Default)]
pub struct OfficeDraft {
    /// Office held or sought (e.g. `vereador`). Required; validated non-blank.
    pub office: String,
    /// Electoral district / municipality, if any.
    pub district: Option<String>,
    /// Term start date, if known.
    pub term_start: Option<NaiveDate>,
    /// Term end date, if known.
    pub term_end: Option<NaiveDate>,
}

/// An identity-assurance binding for a mandate.
#[derive(Debug, Clone)]
pub struct IdentityBinding {
    /// Binding id.
    pub id: Uuid,
    /// The mandate bound.
    pub mandate: MandateId,
    /// The assurance level recorded.
    pub level: VerificationLevel,
    /// Reference to the evidence, if any.
    pub evidence_ref: Option<String>,
    /// When the binding was recorded.
    pub verified_at: DateTime<Utc>,
}

/// The mandate & candidate registry service.
#[derive(Clone)]
pub struct MandateRegistry {
    db: Db,
    clock: Arc<dyn Clock>,
    /// Injected event bus, retained per ADR-0006 for fire-and-forget emission **outside** a domain
    /// transaction. In-transaction emission goes through the transactional outbox instead; this
    /// crate has no fire-and-forget path, hence the explicit accessor below keeps it observable.
    bus: Arc<dyn EventBus>,
    authz: Arc<dyn Authorization>,
}

impl std::fmt::Debug for MandateRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MandateRegistry")
            .field("db", &"PgPool")
            .finish_non_exhaustive()
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

/// Clamp a caller-supplied page size into `[1, MAX_PAGE_LIMIT]`, defaulting when absent.
fn clamp_limit(limit: Option<u32>) -> i64 {
    i64::from(limit.unwrap_or(DEFAULT_PAGE_LIMIT).clamp(1, MAX_PAGE_LIMIT))
}

impl MandateRegistry {
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

    /// The injected event-bus publish port. The registry emits via the transactional outbox, so
    /// this accessor exists to keep the wired fire-and-forget port observable (and future-proof).
    #[must_use]
    pub fn event_bus(&self) -> &Arc<dyn EventBus> {
        &self.bus
    }

    /// Assert the caller may perform a mandate mutation in `org` (never trusts a body-supplied id;
    /// it checks the AUTHENTICATED caller against the injected [`Authorization`]).
    async fn authorize_mutation(&self, org: OrgId, caller: CitizenId) -> Result<()> {
        self.authz.require(org, caller, MIN_MUTATION_LEVEL).await
    }

    /// Invite an official/candidate to onboard via the public email on their mandate. Generates a
    /// high-entropy token, stores ONLY its hash, and emits `mandates.official.invited` through the
    /// outbox — all in one transaction. Returns the one-time plaintext token for delivery.
    ///
    /// # Errors
    /// [`Error::Forbidden`] if the caller is unauthorized; [`Error::NotFound`] if the mandate does
    /// not exist in `org`; [`Error::Storage`]/[`Error::Conflict`] on persistence failure.
    pub async fn invite(
        &self,
        org: OrgId,
        caller: CitizenId,
        mandate: MandateId,
    ) -> Result<Invitation> {
        self.authorize_mutation(org, caller).await?;
        let now = self.clock.now();

        let mut tx = self.db.begin().await.map_err(map_sqlx)?;

        let mandate_row = queries::find_mandate(&mut *tx, org.as_uuid(), mandate.as_uuid())
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| Error::NotFound("mandate not found".to_owned()))?;

        let token = domain::new_invite_token();
        let inserted = queries::insert_invitation(
            &mut *tx,
            Uuid::now_v7(),
            mandate.as_uuid(),
            &mandate_row.public_email,
            &token,
            now,
            now,
        )
        .await
        .map_err(map_sqlx)?;

        // Transactional outbox (ADR-0006): the event lands in `events_log` inside this same
        // transaction, so the invitation and its event commit atomically.
        let envelope = events::official_invited_envelope(self.clock.as_ref(), org, mandate);
        dsoc_db::outbox::publish_tx(&mut *tx, &envelope)
            .await
            .map_err(map_sqlx)?;

        tx.commit().await.map_err(map_sqlx)?;

        Ok(Invitation {
            id: inserted.id,
            mandate,
            public_email: mandate_row.public_email,
            token,
            sent_at: inserted.sent_at,
        })
    }

    /// Accept an invite by presenting the plaintext token. On success: the invitation is marked
    /// accepted, the mandate transitions to onboarded (guarded by `onboarded_at IS NULL`), a
    /// directory-grade identity binding is recorded, and `mandates.official.onboarded` is emitted
    /// through the outbox — atomically. The token is matched by hash; the plaintext is never stored.
    ///
    /// # Errors
    /// - [`Error::Validation`] for a blank or expired token (no state change).
    /// - [`Error::NotFound`] for an unknown token (no state change).
    /// - [`Error::Conflict`] if the invitation was already accepted or the mandate already
    ///   onboarded (double-onboard).
    /// - [`Error::Storage`] on persistence failure.
    pub async fn accept_invitation(&self, org: OrgId, token: &str) -> Result<Onboarding> {
        domain::validate_token(token)?;
        let now = self.clock.now();

        let mut tx = self.db.begin().await.map_err(map_sqlx)?;

        // Lock the invitation + mandate rows for the rest of the transaction (closes the TOCTOU
        // between checking and transitioning). Unknown token -> NotFound, no state change.
        let lock = queries::lock_invitation_by_token(&mut *tx, token, org.as_uuid())
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| Error::NotFound("invitation not found".to_owned()))?;

        // Expired token -> Validation, no state change.
        if domain::is_invitation_expired(lock.sent_at, now, INVITATION_TTL_HOURS) {
            tx.rollback().await.map_err(map_sqlx)?;
            return Err(Error::Validation("invitation has expired".to_owned()));
        }
        // Already-accepted invitation or already-onboarded mandate -> Conflict (double-onboard).
        if lock.accepted_at.is_some() {
            tx.rollback().await.map_err(map_sqlx)?;
            return Err(Error::Conflict("invitation already accepted".to_owned()));
        }
        if lock.onboarded_at.is_some() {
            tx.rollback().await.map_err(map_sqlx)?;
            return Err(Error::Conflict("mandate already onboarded".to_owned()));
        }

        // Mark the invitation accepted under its `accepted_at IS NULL` guard.
        let accepted = queries::mark_invitation_accepted(&mut *tx, lock.invitation_id, now)
            .await
            .map_err(map_sqlx)?;
        if accepted == 0 {
            // A concurrent acceptance won the race: already in the target state.
            tx.rollback().await.map_err(map_sqlx)?;
            return Err(Error::Conflict("invitation already accepted".to_owned()));
        }

        // Transition the mandate to onboarded under its `onboarded_at IS NULL` guard.
        let onboarded = queries::mark_mandate_onboarded(&mut *tx, lock.mandate_id, now)
            .await
            .map_err(map_sqlx)?;
        if onboarded == 0 {
            // Already onboarded via another invite: treat as the already-in-target Conflict.
            tx.rollback().await.map_err(map_sqlx)?;
            return Err(Error::Conflict("mandate already onboarded".to_owned()));
        }

        // Record the evidence binding (directory grade: the official proved control of the email).
        let evidence = format!("invitation:{}", lock.invitation_id);
        queries::insert_identity_binding(
            &mut *tx,
            Uuid::now_v7(),
            lock.mandate_id,
            domain::level_as_str(ONBOARDING_BINDING_LEVEL),
            Some(&evidence),
            now,
            now,
        )
        .await
        .map_err(map_sqlx)?;

        // Transactional outbox: emit onboarded atomically with the transition.
        let mandate = MandateId::from_uuid(lock.mandate_id);
        let envelope = events::official_onboarded_envelope(self.clock.as_ref(), org, mandate);
        dsoc_db::outbox::publish_tx(&mut *tx, &envelope)
            .await
            .map_err(map_sqlx)?;

        tx.commit().await.map_err(map_sqlx)?;

        Ok(Onboarding {
            mandate,
            onboarded_at: now,
        })
    }

    /// Record an identity-assurance binding for a mandate at `level` (e.g. a TSE-backed upgrade),
    /// emitting `mandates.identity.verified` through the outbox. Appends to the audit trail.
    ///
    /// # Errors
    /// [`Error::Forbidden`] if unauthorized; [`Error::NotFound`] if the mandate is absent;
    /// [`Error::Storage`] on persistence failure.
    pub async fn bind_identity(
        &self,
        org: OrgId,
        caller: CitizenId,
        mandate: MandateId,
        level: VerificationLevel,
        evidence_ref: Option<&str>,
    ) -> Result<IdentityBinding> {
        self.authorize_mutation(org, caller).await?;
        let now = self.clock.now();
        let evidence = match evidence_ref {
            Some(raw) => Some(domain::validate_text("evidence_ref", raw)?),
            None => None,
        };

        let mut tx = self.db.begin().await.map_err(map_sqlx)?;

        queries::find_mandate(&mut *tx, org.as_uuid(), mandate.as_uuid())
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| Error::NotFound("mandate not found".to_owned()))?;

        let row = queries::insert_identity_binding(
            &mut *tx,
            Uuid::now_v7(),
            mandate.as_uuid(),
            domain::level_as_str(level),
            evidence.as_deref(),
            now,
            now,
        )
        .await
        .map_err(map_sqlx)?;

        let envelope = events::identity_verified_envelope(self.clock.as_ref(), org, mandate);
        dsoc_db::outbox::publish_tx(&mut *tx, &envelope)
            .await
            .map_err(map_sqlx)?;

        tx.commit().await.map_err(map_sqlx)?;
        map_binding(row)
    }

    /// Add a term-bound office record to a mandate. No cross-crate event (offices are descriptive).
    ///
    /// # Errors
    /// [`Error::Forbidden`] if unauthorized; [`Error::Validation`] on blank office; [`Error::NotFound`]
    /// if the mandate is absent; [`Error::Storage`] on persistence failure.
    pub async fn add_office(
        &self,
        org: OrgId,
        caller: CitizenId,
        mandate: MandateId,
        draft: OfficeDraft,
    ) -> Result<Office> {
        self.authorize_mutation(org, caller).await?;
        let office = domain::validate_text("office", &draft.office)?;
        let district = match draft.district.as_deref() {
            Some(raw) => Some(domain::validate_text("district", raw)?),
            None => None,
        };
        let now = self.clock.now();

        // Verify the mandate exists in this org before adding an office (avoids dangling rows when
        // the mandate id is wrong; the FK alone would surface as an opaque storage error).
        queries::find_mandate(&self.db, org.as_uuid(), mandate.as_uuid())
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| Error::NotFound("mandate not found".to_owned()))?;

        let row = queries::insert_office(
            &self.db,
            Uuid::now_v7(),
            mandate.as_uuid(),
            &office,
            district.as_deref(),
            draft.term_start,
            draft.term_end,
            now,
        )
        .await
        .map_err(map_sqlx)?;
        Ok(office_from_row(row))
    }

    /// Fetch a mandate with its DERIVED onboarding status and public handle (read-only).
    ///
    /// # Errors
    /// [`Error::NotFound`] if absent; [`Error::Storage`] on persistence failure.
    pub async fn get_mandate(&self, org: OrgId, mandate: MandateId) -> Result<MandateView> {
        let row = queries::find_mandate(&self.db, org.as_uuid(), mandate.as_uuid())
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| Error::NotFound("mandate not found".to_owned()))?;

        let has_invitation = queries::mandate_has_invitation(&self.db, row.id)
            .await
            .map_err(map_sqlx)?;
        let status = domain::onboarding_status(row.onboarded_at, has_invitation);
        let id = MandateId::from_uuid(row.id);
        Ok(MandateView {
            id,
            office: row.office,
            display_name: row.display_name,
            is_candidate: row.is_candidate,
            status,
            public_handle: domain::public_handle(id),
            party: row.party,
            uf: row.uf,
            house: row.house,
            avatar_object_key: row.avatar_object_key,
        })
    }

    /// Reverse-lookup: given a citizen, find the mandate they OPERATE (the binding from
    /// migration 0202). Drives the "Painel do mandato" — if `Ok(None)`, the citizen is just a
    /// regular voter; otherwise the painel shows their SLA queue. Returns the binding level
    /// alongside so the painel can display "verificado(a) pelo diretório".
    ///
    /// # Errors
    /// [`Error::Storage`] on persistence failure.
    pub async fn find_my_mandate(
        &self,
        org: OrgId,
        citizen: dsoc_core::ids::CitizenId,
    ) -> Result<Option<(MandateView, String)>> {
        let pair = queries::find_mandate_by_operator(&self.db, org.as_uuid(), citizen.as_uuid())
            .await
            .map_err(map_sqlx)?;
        let Some((row, level)) = pair else { return Ok(None) };
        let has_invitation = queries::mandate_has_invitation(&self.db, row.id)
            .await
            .map_err(map_sqlx)?;
        let status = domain::onboarding_status(row.onboarded_at, has_invitation);
        let id = MandateId::from_uuid(row.id);
        Ok(Some((
            MandateView {
                id,
                office: row.office,
                display_name: row.display_name,
                is_candidate: row.is_candidate,
                status,
                public_handle: domain::public_handle(id),
                party: row.party,
                uf: row.uf,
                house: row.house,
                avatar_object_key: row.avatar_object_key,
            },
            level,
        )))
    }

    /// List mandates in an organization, alphabetical by `display_name`. Public read (no auth)
    /// because the directory of mandates is itself the citizens' map of who they can address; a
    /// hidden mandate would defeat the platform's accountability promise. Drives the front-end
    /// picker on `/propor`.
    ///
    /// # Errors
    /// [`Error::Storage`] on persistence failure.
    pub async fn list_mandates(
        &self,
        org: OrgId,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<MandateView>> {
        let limit = clamp_limit(limit);
        let offset = offset.unwrap_or(0) as i64;
        let rows = queries::list_mandates(&self.db, org.as_uuid(), limit, offset)
            .await
            .map_err(map_sqlx)?;
        let mut views = Vec::with_capacity(rows.len());
        for row in rows {
            // Same derived status as `get_mandate`: collapse the (onboarded_at, has_invitation)
            // pair into the public OnboardingStatus the contract carries.
            let has_invitation = queries::mandate_has_invitation(&self.db, row.id)
                .await
                .map_err(map_sqlx)?;
            let status = domain::onboarding_status(row.onboarded_at, has_invitation);
            let id = MandateId::from_uuid(row.id);
            views.push(MandateView {
                id,
                office: row.office,
                display_name: row.display_name,
                is_candidate: row.is_candidate,
                status,
                public_handle: domain::public_handle(id),
                party: row.party,
                uf: row.uf,
                house: row.house,
                avatar_object_key: row.avatar_object_key,
            });
        }
        Ok(views)
    }

    /// Keyset-paginated office list for a mandate (ordered by id ascending).
    ///
    /// # Errors
    /// [`Error::Storage`] on persistence failure.
    pub async fn list_offices(
        &self,
        mandate: MandateId,
        after: Option<Uuid>,
        limit: Option<u32>,
    ) -> Result<Vec<Office>> {
        let rows = queries::list_offices(&self.db, mandate.as_uuid(), after, clamp_limit(limit))
            .await
            .map_err(map_sqlx)?;
        Ok(rows.into_iter().map(office_from_row).collect())
    }

    /// Keyset-paginated identity-binding history for a mandate, newest first.
    ///
    /// # Errors
    /// [`Error::Storage`] on persistence failure (or a corrupt level string).
    pub async fn list_identity_bindings(
        &self,
        mandate: MandateId,
        cursor: Option<(DateTime<Utc>, Uuid)>,
        limit: Option<u32>,
    ) -> Result<Vec<IdentityBinding>> {
        let rows = queries::list_identity_bindings(
            &self.db,
            mandate.as_uuid(),
            cursor,
            clamp_limit(limit),
        )
        .await
        .map_err(map_sqlx)?;
        rows.into_iter().map(map_binding).collect()
    }

    /// Handle an inbound cross-crate event (idempotent). Returns whether it was consumed.
    ///
    /// # Errors
    /// Propagates any error from the consume handler.
    pub fn consume(&self, envelope: &EventEnvelope) -> Result<bool> {
        events::on_event(envelope)
    }
}

/// Map an office row to its public domain type.
fn office_from_row(row: OfficeRow) -> Office {
    Office {
        id: row.id,
        mandate: MandateId::from_uuid(row.mandate_id),
        office: row.office,
        district: row.district,
        term_start: row.term_start,
        term_end: row.term_end,
        created_at: row.created_at,
    }
}

/// Map an identity-binding row to its public domain type, parsing the stored level string.
fn map_binding(row: IdentityBindingRow) -> Result<IdentityBinding> {
    Ok(IdentityBinding {
        id: row.id,
        mandate: MandateId::from_uuid(row.mandate_id),
        level: domain::level_from_str(&row.verification_level)?,
        evidence_ref: row.evidence_ref,
        verified_at: row.verified_at,
    })
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
    fn clamp_limit_defaults_and_bounds() {
        assert_eq!(clamp_limit(None), i64::from(DEFAULT_PAGE_LIMIT));
        assert_eq!(clamp_limit(Some(0)), 1);
        assert_eq!(clamp_limit(Some(10)), 10);
        assert_eq!(clamp_limit(Some(10_000)), i64::from(MAX_PAGE_LIMIT));
    }
}

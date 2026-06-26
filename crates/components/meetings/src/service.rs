//! The meetings service: holds the pool and the injected [`Clock`]. It schedules meetings,
//! records attendance/RSVPs (idempotently), sets minutes, and reads everything back with keyset
//! pagination. All failures map onto the canonical [`dsoc_core::Error`].
//!
//! This domain emits **no** cross-crate events: `meetings.*` is not part of the frozen
//! [`dsoc_core::events::Event`] catalog and nothing consumes it, so (like `dsoc-admin`) the crate
//! keeps its state private and exposes it only through its routes. Time is read only from the
//! injected [`Clock`], never ambiently (TESTING.md).
//!
//! There is no `MeetingId` in the frozen `dsoc_core::ids`, and a meeting id never crosses a crate
//! boundary (no peer consumes it), so the meeting identity is a bare `uuid::Uuid` here; the
//! cross-boundary identities (`org`, `citizen`) remain the typed core newtypes.

use std::sync::Arc;

use dsoc_core::ids::{CitizenId, OrgId};
use dsoc_core::{Clock, Error, Result};
use dsoc_db::Db;
use uuid::Uuid;

use crate::domain::{NewMeeting, ValidMinutes};
use crate::queries::{self, AttendeeRow, MeetingRow};

/// Default page size for listings.
pub const DEFAULT_PAGE_LIMIT: i64 = 20;
/// Hard cap on page size to keep reads bounded.
pub const MAX_PAGE_LIMIT: i64 = 100;

/// Civic-meetings service. Cheap to construct per request from `AppState`.
#[derive(Clone)]
pub struct MeetingService {
    db: Db,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for MeetingService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeetingService").finish_non_exhaustive()
    }
}

impl MeetingService {
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

    /// Schedule a meeting owned by `org`. The created-at stamp comes from the injected clock; the
    /// meeting's `starts_at` is the caller-supplied instant carried through validation.
    ///
    /// # Errors
    /// - [`Error::Conflict`] on a unique violation.
    /// - [`Error::Validation`] on a foreign-key/check violation (e.g. unknown org).
    /// - [`Error::Storage`] on any other persistence failure.
    pub async fn schedule(&self, org: OrgId, new: &NewMeeting) -> Result<MeetingRow> {
        queries::insert_meeting(
            &self.db,
            Uuid::now_v7(),
            org.as_uuid(),
            &new.title,
            &new.location,
            new.starts_at,
            self.clock.now(),
        )
        .await
        .map_err(map_sqlx)
    }

    /// Fetch a single meeting.
    ///
    /// # Errors
    /// [`Error::NotFound`] when absent; [`Error::Storage`] otherwise.
    pub async fn get_meeting(&self, meeting: Uuid) -> Result<MeetingRow> {
        queries::get_meeting(&self.db, meeting)
            .await
            .map_err(map_sqlx)
    }

    /// List an org's meetings with keyset pagination, returning the rows and the total count.
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn list_meetings(
        &self,
        org: OrgId,
        after: Option<Uuid>,
        limit: i64,
    ) -> Result<(Vec<MeetingRow>, i64)> {
        let rows = queries::list_meetings(&self.db, org.as_uuid(), after, clamp_limit(limit))
            .await
            .map_err(map_sqlx)?;
        let total = queries::count_meetings(&self.db, org.as_uuid())
            .await
            .map_err(map_sqlx)?;
        Ok((rows, total))
    }

    /// Record `citizen` attending `meeting` (an RSVP). The meeting must exist (a missing meeting is
    /// a clean [`Error::NotFound`] for the path id, never a raw FK error). The write is idempotent:
    /// a repeat RSVP returns the already-stored attendance row with its real id, stamped from the
    /// injected clock on first insert.
    ///
    /// # Errors
    /// - [`Error::NotFound`] when the meeting does not exist.
    /// - [`Error::Validation`] on a foreign-key violation (e.g. unknown citizen).
    /// - [`Error::Storage`] on any other persistence failure.
    pub async fn attend(&self, meeting: Uuid, citizen: CitizenId) -> Result<AttendeeRow> {
        // Confirm the meeting exists so a missing path id is a 404, not a generic FK error.
        self.get_meeting(meeting).await?;
        queries::upsert_attendee(
            &self.db,
            Uuid::now_v7(),
            meeting,
            citizen.as_uuid(),
            self.clock.now(),
        )
        .await
        .map_err(map_sqlx)
    }

    /// List a meeting's attendees with keyset pagination, returning the rows and total.
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn list_attendees(
        &self,
        meeting: Uuid,
        after: Option<Uuid>,
        limit: i64,
    ) -> Result<(Vec<AttendeeRow>, i64)> {
        let rows = queries::list_attendees(&self.db, meeting, after, clamp_limit(limit))
            .await
            .map_err(map_sqlx)?;
        let total = queries::count_attendees(&self.db, meeting)
            .await
            .map_err(map_sqlx)?;
        Ok((rows, total))
    }

    /// Set (replace) a meeting's minutes, returning the updated row. A missing meeting is a clean
    /// [`Error::NotFound`] for the path id.
    ///
    /// # Errors
    /// - [`Error::NotFound`] when the meeting does not exist.
    /// - [`Error::Validation`] on a check violation.
    /// - [`Error::Storage`] on any other persistence failure.
    pub async fn set_minutes(&self, meeting: Uuid, minutes: &ValidMinutes) -> Result<MeetingRow> {
        queries::update_minutes(&self.db, meeting, minutes.as_str())
            .await
            .map_err(map_sqlx)
    }
}

/// Clamp a requested page size into `1..=MAX_PAGE_LIMIT`.
fn clamp_limit(limit: i64) -> i64 {
    limit.clamp(1, MAX_PAGE_LIMIT)
}

/// Map a `sqlx` failure onto the canonical error model (CONTRIBUTING.md wiring conventions):
/// missing row -> `NotFound`, unique violation -> `Conflict`, foreign-key/check violation ->
/// `Validation`, everything else -> `Storage` (logged server-side, never surfaced raw).
fn map_sqlx(err: sqlx::Error) -> Error {
    match err {
        sqlx::Error::RowNotFound => Error::NotFound("meeting not found".to_string()),
        sqlx::Error::Database(ref db) if db.is_unique_violation() => {
            Error::Conflict("attendance already recorded".to_string())
        }
        sqlx::Error::Database(ref db) if db.is_foreign_key_violation() => {
            Error::Validation("referenced org, meeting or citizen does not exist".to_string())
        }
        sqlx::Error::Database(ref db) if db.is_check_violation() => {
            Error::Validation("input violates a constraint".to_string())
        }
        other => Error::Storage(Box::new(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_limit_bounds_page_size() {
        assert_eq!(clamp_limit(0), 1);
        assert_eq!(clamp_limit(-10), 1);
        assert_eq!(clamp_limit(20), 20);
        assert_eq!(clamp_limit(10_000), MAX_PAGE_LIMIT);
    }

    #[test]
    fn map_sqlx_row_not_found_is_not_found() {
        assert_eq!(map_sqlx(sqlx::Error::RowNotFound).code(), "not_found");
    }

    #[test]
    fn map_sqlx_other_is_storage() {
        assert_eq!(map_sqlx(sqlx::Error::PoolTimedOut).code(), "storage_error");
    }
}

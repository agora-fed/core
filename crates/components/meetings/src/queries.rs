//! Every database statement for meetings, as `sqlx` compile-time-checked queries (PLAN.md
//! principle 3 — no ORM, no `SELECT *`, keyset pagination on unbounded reads).
//!
//! Row shapes are returned to the service, which maps `sqlx` failures onto the canonical
//! [`dsoc_core::Error`]. Inserts use `RETURNING` so the caller always gets the real stored row
//! (never a phantom pre-generated value). The attendance insert is idempotent — a repeat RSVP
//! hits the `UNIQUE(meeting_id, citizen_id)` constraint, an `ON CONFLICT ... DO UPDATE` re-emits
//! the existing row, and the caller still receives the real stored id. Lists keyset-paginate over
//! the ascending UUIDv7 `id` (time-ordered), scoped by their parent.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// A persisted meeting row (`meeting`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingRow {
    /// Meeting id.
    pub id: Uuid,
    /// Owning organization.
    pub org_id: Uuid,
    /// Title — what the meeting is about.
    pub title: String,
    /// Location — where it happens.
    pub location: String,
    /// When the meeting begins.
    pub starts_at: DateTime<Utc>,
    /// Recorded minutes, absent until set after the meeting.
    pub minutes: Option<String>,
    /// Creation time (from the injected clock).
    pub created_at: DateTime<Utc>,
}

/// A persisted attendance row (`meeting_attendee`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttendeeRow {
    /// Attendance id.
    pub id: Uuid,
    /// The meeting attended.
    pub meeting_id: Uuid,
    /// The attending citizen.
    pub citizen_id: Uuid,
    /// When the attendance/RSVP was recorded.
    pub created_at: DateTime<Utc>,
}

/// Insert a meeting and return the stored row.
///
/// # Errors
/// Propagates the underlying `sqlx::Error` (e.g. unknown `org_id`, or a CHECK violation).
pub async fn insert_meeting(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    org_id: Uuid,
    title: &str,
    location: &str,
    starts_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
) -> Result<MeetingRow, sqlx::Error> {
    let row = sqlx::query!(
        r#"INSERT INTO meeting (id, org_id, title, location, starts_at, minutes, created_at)
           VALUES ($1, $2, $3, $4, $5, NULL, $6)
           RETURNING id, org_id, title, location, starts_at, minutes, created_at"#,
        id,
        org_id,
        title,
        location,
        starts_at,
        created_at,
    )
    .fetch_one(executor)
    .await?;
    Ok(MeetingRow {
        id: row.id,
        org_id: row.org_id,
        title: row.title,
        location: row.location,
        starts_at: row.starts_at,
        minutes: row.minutes,
        created_at: row.created_at,
    })
}

/// Fetch a single meeting by id.
///
/// # Errors
/// Propagates the underlying `sqlx::Error` (including `RowNotFound` when absent).
pub async fn get_meeting(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
) -> Result<MeetingRow, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT id, org_id, title, location, starts_at, minutes, created_at
           FROM meeting
           WHERE id = $1"#,
        id,
    )
    .fetch_one(executor)
    .await?;
    Ok(MeetingRow {
        id: row.id,
        org_id: row.org_id,
        title: row.title,
        location: row.location,
        starts_at: row.starts_at,
        minutes: row.minutes,
        created_at: row.created_at,
    })
}

/// List an org's meetings with keyset pagination over ascending `id` (UUIDv7 = time-ordered).
/// `after` is the last id seen; `None` starts from the beginning.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn list_meetings(
    executor: impl sqlx::PgExecutor<'_>,
    org_id: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<MeetingRow>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT id, org_id, title, location, starts_at, minutes, created_at
           FROM meeting
           WHERE org_id = $1 AND ($2::uuid IS NULL OR id > $2)
           ORDER BY id
           LIMIT $3"#,
        org_id,
        after,
        limit,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| MeetingRow {
            id: row.id,
            org_id: row.org_id,
            title: row.title,
            location: row.location,
            starts_at: row.starts_at,
            minutes: row.minutes,
            created_at: row.created_at,
        })
        .collect())
}

/// Count the meetings owned by an org (for pagination metadata).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn count_meetings(
    executor: impl sqlx::PgExecutor<'_>,
    org_id: Uuid,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT count(*) AS "count!: i64" FROM meeting WHERE org_id = $1"#,
        org_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(row.count)
}

/// Set (replace) a meeting's minutes, returning the updated row.
///
/// # Errors
/// Propagates the underlying `sqlx::Error` (including `RowNotFound` when the meeting is absent).
pub async fn update_minutes(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    minutes: &str,
) -> Result<MeetingRow, sqlx::Error> {
    let row = sqlx::query!(
        r#"UPDATE meeting
           SET minutes = $2
           WHERE id = $1
           RETURNING id, org_id, title, location, starts_at, minutes, created_at"#,
        id,
        minutes,
    )
    .fetch_one(executor)
    .await?;
    Ok(MeetingRow {
        id: row.id,
        org_id: row.org_id,
        title: row.title,
        location: row.location,
        starts_at: row.starts_at,
        minutes: row.minutes,
        created_at: row.created_at,
    })
}

/// Record a citizen's attendance for a meeting, idempotently. A repeat RSVP returns the
/// already-stored row (the `DO UPDATE` lets `RETURNING` surface the existing id) rather than
/// erroring on the `UNIQUE(meeting_id, citizen_id)` constraint.
///
/// # Errors
/// Propagates the underlying `sqlx::Error` (e.g. unknown `meeting_id`/`citizen_id`).
pub async fn upsert_attendee(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    meeting_id: Uuid,
    citizen_id: Uuid,
    created_at: DateTime<Utc>,
) -> Result<AttendeeRow, sqlx::Error> {
    let row = sqlx::query!(
        r#"INSERT INTO meeting_attendee (id, meeting_id, citizen_id, created_at)
           VALUES ($1, $2, $3, $4)
           ON CONFLICT (meeting_id, citizen_id)
               DO UPDATE SET meeting_id = EXCLUDED.meeting_id
           RETURNING id, meeting_id, citizen_id, created_at"#,
        id,
        meeting_id,
        citizen_id,
        created_at,
    )
    .fetch_one(executor)
    .await?;
    Ok(AttendeeRow {
        id: row.id,
        meeting_id: row.meeting_id,
        citizen_id: row.citizen_id,
        created_at: row.created_at,
    })
}

/// List a meeting's attendees with keyset pagination over ascending `id` (oldest-first).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn list_attendees(
    executor: impl sqlx::PgExecutor<'_>,
    meeting_id: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<AttendeeRow>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT id, meeting_id, citizen_id, created_at
           FROM meeting_attendee
           WHERE meeting_id = $1 AND ($2::uuid IS NULL OR id > $2)
           ORDER BY id
           LIMIT $3"#,
        meeting_id,
        after,
        limit,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| AttendeeRow {
            id: row.id,
            meeting_id: row.meeting_id,
            citizen_id: row.citizen_id,
            created_at: row.created_at,
        })
        .collect())
}

/// Count the attendees of a meeting (for pagination metadata).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn count_attendees(
    executor: impl sqlx::PgExecutor<'_>,
    meeting_id: Uuid,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT count(*) AS "count!: i64" FROM meeting_attendee WHERE meeting_id = $1"#,
        meeting_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(row.count)
}

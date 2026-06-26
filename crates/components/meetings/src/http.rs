//! Axum surface for `meetings`. Handlers map the domain onto `api-contract`'s [`ApiResponse`]
//! envelope and never bind a socket — the gateway owns the IPv6 bind (ADR-0004).
//!
//! **Authorization (SECURITY.md / ADR-0007).** Every *mutating* handler authorizes the
//! authenticated caller through the verified [`dsoc_app::CallerId`] extractor — the acting citizen
//! and org are taken from the request's identity, NEVER from the body — and requires at least an
//! email-confirmed citizen via `authz.require` before any write. Reads are public.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use dsoc_api_contract::{ApiResponse, PageMeta};
use dsoc_app::{AppState, CallerId};
use dsoc_core::ids::OrgId;
use dsoc_core::{Error, VerificationLevel};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::domain::{NewMeeting, ValidMinutes};
use crate::queries::{AttendeeRow, MeetingRow};
use crate::service::{MeetingService, DEFAULT_PAGE_LIMIT};

/// Minimum assurance required to schedule, RSVP, or set minutes: an email-confirmed citizen.
const REQUIRED_LEVEL: VerificationLevel = VerificationLevel::Email;

/// The crate's router. Mounted by the gateway; takes the frozen [`dsoc_app::AppState`].
pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/meetings", post(schedule).get(list_meetings))
        .route("/meetings/{id}", get(get_meeting))
        .route("/meetings/{id}/minutes", put(set_minutes))
        .route("/meetings/{id}/attendees", post(attend).get(list_attendees))
        .with_state(state)
}

// --- DTOs ----------------------------------------------------------------------------

/// Public view of a meeting.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MeetingDto {
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
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

impl From<MeetingRow> for MeetingDto {
    fn from(row: MeetingRow) -> Self {
        Self {
            id: row.id,
            org_id: row.org_id,
            title: row.title,
            location: row.location,
            starts_at: row.starts_at,
            minutes: row.minutes,
            created_at: row.created_at,
        }
    }
}

/// Public view of a meeting attendee.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AttendeeDto {
    /// Attendance id.
    pub id: Uuid,
    /// The meeting attended.
    pub meeting_id: Uuid,
    /// The attending citizen.
    pub citizen_id: Uuid,
    /// When the attendance/RSVP was recorded.
    pub created_at: DateTime<Utc>,
}

impl From<AttendeeRow> for AttendeeDto {
    fn from(row: AttendeeRow) -> Self {
        Self {
            id: row.id,
            meeting_id: row.meeting_id,
            citizen_id: row.citizen_id,
            created_at: row.created_at,
        }
    }
}

/// Request to schedule a meeting. The acting org comes from the verified [`dsoc_app::CallerId`]
/// (ADR-0007), never the body.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct ScheduleRequest {
    /// Title — what the meeting is about.
    pub title: String,
    /// Location — a room, an address, or an online URL.
    pub location: String,
    /// When the meeting begins.
    pub starts_at: DateTime<Utc>,
}

/// Request to set a meeting's minutes.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct SetMinutesRequest {
    /// The minutes text (Portuguese civic content).
    pub minutes: String,
}

/// Query parameters for listing meetings (keyset pagination over ascending id).
#[derive(Debug, Clone, Deserialize)]
pub struct ListParams {
    /// Owning organization to scope the listing.
    pub org_id: Uuid,
    /// Keyset cursor: return meetings with id greater than this. Omit for the first page.
    pub after: Option<Uuid>,
    /// Page size (clamped server-side).
    pub limit: Option<i64>,
}

/// Query parameters for listing a meeting's attendees.
#[derive(Debug, Clone, Deserialize)]
pub struct AttendeeListParams {
    /// Keyset cursor over ascending attendance id. Omit for the first page.
    pub after: Option<Uuid>,
    /// Page size (clamped server-side).
    pub limit: Option<i64>,
}

// --- handlers ------------------------------------------------------------------------

/// `POST /meetings` — schedule a meeting owned by the caller's org.
///
/// Scheduling is a citizen action, so the verified caller (from [`dsoc_app::CallerId`], never the
/// body — ADR-0007) must be at least email-verified within its org before any write.
async fn schedule(
    State(state): State<AppState>,
    caller: CallerId,
    Json(req): Json<ScheduleRequest>,
) -> Response {
    if let Err(e) = state
        .authz
        .require(caller.org, caller.citizen, REQUIRED_LEVEL)
        .await
    {
        return error_response::<MeetingDto>(&e);
    }
    let new = match NewMeeting::validate(&req.title, &req.location, req.starts_at) {
        Ok(n) => n,
        Err(e) => return error_response::<MeetingDto>(&e),
    };
    let svc = MeetingService::from_state(&state);
    match svc.schedule(caller.org, &new).await {
        Ok(row) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(MeetingDto::from(row))),
        )
            .into_response(),
        Err(e) => error_response::<MeetingDto>(&e),
    }
}

/// `GET /meetings/{id}` — fetch one meeting (public read).
async fn get_meeting(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    let svc = MeetingService::from_state(&state);
    match svc.get_meeting(id).await {
        Ok(row) => (StatusCode::OK, Json(ApiResponse::ok(MeetingDto::from(row)))).into_response(),
        Err(e) => error_response::<MeetingDto>(&e),
    }
}

/// `GET /meetings?org_id=&after=&limit=` — keyset-paginated meeting list (public read).
async fn list_meetings(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> Response {
    let svc = MeetingService::from_state(&state);
    let org = OrgId::from_uuid(params.org_id);
    let limit = params.limit.unwrap_or(DEFAULT_PAGE_LIMIT);
    match svc.list_meetings(org, params.after, limit).await {
        Ok((rows, total)) => {
            let dtos: Vec<MeetingDto> = rows.into_iter().map(MeetingDto::from).collect();
            (
                StatusCode::OK,
                Json(ApiResponse::page(dtos, page_meta(total, limit))),
            )
                .into_response()
        }
        Err(e) => error_response::<Vec<MeetingDto>>(&e),
    }
}

/// `PUT /meetings/{id}/minutes` — set (replace) a meeting's minutes.
///
/// Recording minutes is a citizen action; the verified caller (from [`dsoc_app::CallerId`], never
/// the body — ADR-0007) is authorized (email-verified) before any write.
async fn set_minutes(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<Uuid>,
    Json(req): Json<SetMinutesRequest>,
) -> Response {
    if let Err(e) = state
        .authz
        .require(caller.org, caller.citizen, REQUIRED_LEVEL)
        .await
    {
        return error_response::<MeetingDto>(&e);
    }
    let minutes = match ValidMinutes::validate(&req.minutes) {
        Ok(m) => m,
        Err(e) => return error_response::<MeetingDto>(&e),
    };
    let svc = MeetingService::from_state(&state);
    match svc.set_minutes(id, &minutes).await {
        Ok(row) => (StatusCode::OK, Json(ApiResponse::ok(MeetingDto::from(row)))).into_response(),
        Err(e) => error_response::<MeetingDto>(&e),
    }
}

/// `POST /meetings/{id}/attendees` — RSVP/attend a meeting.
///
/// Attending is a citizen action; the verified caller (from [`dsoc_app::CallerId`], never the body
/// — ADR-0007) is authorized (email-verified) before any write and recorded as the attendee. The
/// write is idempotent: a repeat RSVP returns the already-stored attendance row.
async fn attend(State(state): State<AppState>, caller: CallerId, Path(id): Path<Uuid>) -> Response {
    if let Err(e) = state
        .authz
        .require(caller.org, caller.citizen, REQUIRED_LEVEL)
        .await
    {
        return error_response::<AttendeeDto>(&e);
    }
    let svc = MeetingService::from_state(&state);
    match svc.attend(id, caller.citizen).await {
        Ok(row) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(AttendeeDto::from(row))),
        )
            .into_response(),
        Err(e) => error_response::<AttendeeDto>(&e),
    }
}

/// `GET /meetings/{id}/attendees?after=&limit=` — keyset-paginated attendees (public read).
async fn list_attendees(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<AttendeeListParams>,
) -> Response {
    let svc = MeetingService::from_state(&state);
    let limit = params.limit.unwrap_or(DEFAULT_PAGE_LIMIT);
    match svc.list_attendees(id, params.after, limit).await {
        Ok((rows, total)) => {
            let dtos: Vec<AttendeeDto> = rows.into_iter().map(AttendeeDto::from).collect();
            (
                StatusCode::OK,
                Json(ApiResponse::page(dtos, page_meta(total, limit))),
            )
                .into_response()
        }
        Err(e) => error_response::<Vec<AttendeeDto>>(&e),
    }
}

// --- helpers -------------------------------------------------------------------------

/// Build pagination metadata, clamping the reported page size into the wire range.
fn page_meta(total: i64, limit: i64) -> PageMeta {
    PageMeta {
        total: u64::try_from(total).unwrap_or_default(),
        limit: u32::try_from(limit.clamp(1, 100)).unwrap_or(DEFAULT_PAGE_LIMIT as u32),
        offset: 0,
    }
}

/// Render a canonical [`Error`] as an envelope with the matching HTTP status, leaking no internals.
fn error_response<T: Serialize>(err: &Error) -> Response {
    let status = status_for(err);
    let body: ApiResponse<T> = ApiResponse::fail(err.code(), err.to_string());
    (status, Json(body)).into_response()
}

/// Map the canonical error model onto HTTP status codes (mirrors the gateway's contract).
fn status_for(err: &Error) -> StatusCode {
    match err {
        Error::NotFound(_) => StatusCode::NOT_FOUND,
        Error::Forbidden(_) => StatusCode::FORBIDDEN,
        Error::Unauthorized => StatusCode::UNAUTHORIZED,
        Error::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
        Error::Conflict(_) => StatusCode::CONFLICT,
        Error::Dependency { .. } => StatusCode::BAD_GATEWAY,
        // `Error` is #[non_exhaustive]; Storage and any future variant are server-side.
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 25, 18, 30, 0).unwrap()
    }

    #[test]
    fn meeting_dto_maps_fields() {
        let row = MeetingRow {
            id: Uuid::now_v7(),
            org_id: Uuid::now_v7(),
            title: "Assembleia".to_string(),
            location: "Praça Central".to_string(),
            starts_at: at(),
            minutes: None,
            created_at: at(),
        };
        let dto = MeetingDto::from(row.clone());
        assert_eq!(dto.id, row.id);
        assert_eq!(dto.title, "Assembleia");
        assert_eq!(dto.location, "Praça Central");
        assert_eq!(dto.starts_at, at());
        assert!(dto.minutes.is_none());
    }

    #[test]
    fn attendee_dto_maps_fields() {
        let row = AttendeeRow {
            id: Uuid::now_v7(),
            meeting_id: Uuid::now_v7(),
            citizen_id: Uuid::now_v7(),
            created_at: at(),
        };
        let dto = AttendeeDto::from(row.clone());
        assert_eq!(dto.meeting_id, row.meeting_id);
        assert_eq!(dto.citizen_id, row.citizen_id);
        assert_eq!(dto.created_at, at());
    }

    #[test]
    fn page_meta_clamps_and_counts() {
        let meta = page_meta(7, 20);
        assert_eq!(meta.total, 7);
        assert_eq!(meta.limit, 20);
        assert_eq!(meta.offset, 0);
        let degenerate = page_meta(-1, 0);
        assert_eq!(degenerate.total, 0);
        assert_eq!(degenerate.limit, 1);
    }

    #[test]
    fn status_codes_match_error_kinds() {
        assert_eq!(
            status_for(&Error::NotFound("x".into())),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status_for(&Error::Forbidden("x".into())),
            StatusCode::FORBIDDEN
        );
        assert_eq!(status_for(&Error::Unauthorized), StatusCode::UNAUTHORIZED);
        assert_eq!(
            status_for(&Error::Validation("x".into())),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            status_for(&Error::Conflict("x".into())),
            StatusCode::CONFLICT
        );
        assert_eq!(
            status_for(&Error::Storage(Box::new(std::io::Error::other("x")))),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}

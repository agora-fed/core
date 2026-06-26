//! Shared data-transfer objects. These are the *public* shapes; internal domain types
//! live in their owning crates and are mapped to these at the gateway boundary.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// Public view of a proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ProposalDto {
    /// Proposal id.
    pub id: Uuid,
    /// Title (Portuguese, civic content).
    pub title: String,
    /// Body / description.
    pub body: String,
    /// The mandate this proposal is directed at.
    pub mandate_id: Uuid,
    /// Consensus cluster this proposal was merged into, if any.
    pub cluster_id: Option<Uuid>,
    /// Aggregate support count (never per-citizen linkage).
    pub support_count: u64,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

/// Public view of a mandate / candidacy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct MandateDto {
    /// Mandate id.
    pub id: Uuid,
    /// Office held or sought.
    pub office: String,
    /// Public display name.
    pub display_name: String,
    /// Whether this is a candidacy (vs sitting office).
    pub is_candidate: bool,
    /// Whether the official has completed onboarding.
    pub onboarded: bool,
}

/// State of a consequence SLA, surfaced to clients (the emotional core of the UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SlaStatus {
    /// Clock running; official notified, awaiting response.
    Pending,
    /// Official responded in time.
    Answered,
    /// Official acted (response plus a concrete commitment).
    Acted,
    /// SLA expired with no response — public silence.
    Ignored,
}

/// The authenticated citizen's own profile (returned by `GET /me`). Sensitive fields
/// (CPF, e-mail) are NEVER part of this DTO — the federation surface and the public face of
/// the citizen never derive from credentials (ADR-0008 / ADR-0010 / LGPD).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ProfileDto {
    /// Opaque internal id.
    pub citizen_id: Uuid,
    /// Org / tenant the citizen belongs to.
    pub org_id: Uuid,
    /// User-chosen federation handle (`@handle@host`), or `None` if not yet picked.
    pub handle: Option<String>,
    /// Stable opaque handle (`u-<hex>`) shown until the citizen picks one.
    pub public_handle: String,
    /// Friendly name shown in the header and on civic content.
    pub display_name: Option<String>,
    /// Short self-description (Portuguese, ≤ 500 chars from the UI; DB allows 1000 as guard).
    pub bio: Option<String>,
    /// Avatar URL (publicly resolvable) — or `None` to fall back to the default rendered SVG.
    pub avatar_url: Option<String>,
    /// Cover image URL — or `None`.
    pub cover_url: Option<String>,
    /// Privacy gate. `false` (default) = local only, federation Actor is NOT materialized.
    pub is_public: bool,
    /// Verification level reached (`anonymous` / `email` / `directory` / `strong`). Cosmetic
    /// badge today; gates some operations elsewhere in the platform.
    pub verification_level: String,
    /// First seen on the platform.
    pub created_at: DateTime<Utc>,
}

/// One active (or expired-but-not-cleaned) session of the authenticated citizen, returned by
/// `GET /me/sessions`. Carries no credentials — just the opaque session id (which the user can
/// then revoke), the timestamps, and a `current` flag so the UI can clearly mark "this device".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SessionInfoDto {
    /// Opaque session id (usable as the path parameter on `DELETE /me/sessions/{id}`).
    pub id: Uuid,
    /// When this session was issued.
    pub issued_at: DateTime<Utc>,
    /// When this session expires (or expired — sessions don't auto-disappear; the cleanup happens
    /// at next use or via this list-and-revoke surface).
    pub expires_at: DateTime<Utc>,
    /// True iff this is the session the request itself was made on. The UI uses it to disable
    /// the revoke button (revoking your current session would be the same as logging out, which
    /// has its own path), or to add an "(este dispositivo)" tag.
    pub current: bool,
}

/// Editable subset of [`ProfileDto`] accepted by `PATCH /me`. Every field is optional so the
/// caller can patch one attribute at a time; `None` means "leave as-is". To CLEAR an optional
/// field (e.g. wipe the bio), send `Some("")` — the service interprets empty strings as `NULL`.
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
pub struct ProfileUpdateDto {
    /// New display name (`""` to clear).
    pub display_name: Option<String>,
    /// New bio (`""` to clear).
    pub bio: Option<String>,
    /// New handle (validated server-side; rejected if already taken in this org).
    pub handle: Option<String>,
    /// Toggle federation visibility. `true` materializes the citizen as an ActivityPub Actor.
    pub is_public: Option<bool>,
}

/// Public per-politician scorecard summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ScorecardDto {
    /// Mandate this scorecard belongs to.
    pub mandate_id: Uuid,
    /// Proposals answered within SLA.
    pub answered: u64,
    /// Proposals ignored (public silence).
    pub ignored: u64,
    /// Median response latency in hours (None if no responses yet).
    pub median_response_hours: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sla_status_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&SlaStatus::Ignored).unwrap(),
            "\"ignored\""
        );
    }
}

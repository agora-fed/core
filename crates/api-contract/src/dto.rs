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
    /// Support count at which the consequence loop fires (resolves SLA clock UI).
    pub threshold: u64,
    /// Author (citizen who proposed). `None` for legacy / platform-seeded rows.
    pub author_handle: Option<String>,
    /// Public opaque handle (`u-<hex>`) of the author. Always present when there IS an author —
    /// the UI falls back to this if the user-chosen handle is unset.
    pub author_public_handle: Option<String>,
    /// Author avatar URL (resolved server-side from `MEDIA_BASE_URL`). `None` if no avatar.
    pub author_avatar_url: Option<String>,
    /// Nível de urgência (0.25.0-fediverso). `comum` (default) ou `urgente` — este
    /// exige `titulo_status ∈ (validated,verified)` do votante (Fatia D). Aditivo,
    /// retrocompatível: front antigo ignora o campo.
    #[serde(default = "default_urgencia")]
    pub urgencia: String,
    /// Ciclo de vida (`draft` | `published` | `clustered`). Draft = ainda em
    /// moderação (não aparece publicamente). Fase A do plano TSE (0.26.0).
    #[serde(default = "default_status")]
    pub status: String,
    /// Quando o moderador liberou (evento `moderation.cleared`). Null enquanto em draft.
    #[serde(default)]
    pub published_at: Option<DateTime<Utc>>,
    /// Recibo de entrega ao autor (0.25.0-fediverso). `Some(ts)` = e-mail de
    /// confirmação já saiu; `None` = ainda não. Populado pelo worker
    /// `proposal-delivery-worker`.
    #[serde(default)]
    pub notified_author_at: Option<DateTime<Utc>>,
    /// Recibo de entrega ao gabinete (mandate.public_email). `Some(ts)` = e-mail
    /// foi entregue ao relay SMTP; `None` = ainda não (ou mandato sem e-mail).
    #[serde(default)]
    pub notified_mandate_at: Option<DateTime<Utc>>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

fn default_urgencia() -> String {
    "comum".to_owned()
}
fn default_status() -> String {
    "published".to_owned()
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
    /// Sigla do partido (e.g. `PT`, `PSOL`). `None` for legacy/fictional mandates.
    pub party: Option<String>,
    /// Sigla da UF (e.g. `BA`, `SP`). `None` when not applicable.
    pub uf: Option<String>,
    /// Casa: `camara` | `senado`. `None` for legacy.
    pub house: Option<String>,
    /// Avatar URL (resolved via `MEDIA_BASE_URL` from the stored object key). `None` ⇒ UI
    /// shows initials.
    pub avatar_url: Option<String>,
    /// Public office e-mail (the address citizens can write to). `None` when redacted or
    /// unknown. Publicly exposed by design — accountability contact channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_email: Option<String>,
    /// Federative sphere: `federal` | `estadual` | `municipal` (migration 0203).
    /// Legacy DTOs that predate the field will default to `federal` under Serde.
    #[serde(default = "default_sphere")]
    pub sphere: String,
    /// Whether this mandate has a **verified operator** bound (an entry in
    /// `mandate_identity_binding` with a non-null `citizen_id`). Aggregate signal only — the
    /// operator's identity itself never appears on the public surface (LGPD; that lives on the
    /// auth-only `MyMandateDto.binding_level`). Drives the "vínculo verificado" badge.
    ///
    /// `#[serde(default)]` so a legacy client parsing a fresh payload — or a fresh client
    /// parsing a legacy payload — treats the missing field as `false` (the safe fallback).
    #[serde(default)]
    pub has_verified_operator: bool,
}

fn default_sphere() -> String {
    "federal".to_owned()
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
    /// Status do título de eleitor (0.25.0-fediverso): `unverified` (submetido, sem
    /// cross-check), `validated` (dígitos TSE OK), `verified` (cross-check TSE), ou `None`
    /// (nunca cadastrado). Sinaliza cidadania política brasileira — gate pra pauta urgente.
    #[serde(default)]
    pub titulo_status: Option<String>,
    /// First seen on the platform.
    pub created_at: DateTime<Utc>,
}

/// What `GET /me/mandate` returns — the mandate the authenticated citizen operates, if any.
/// Drives the "Painel do mandato" page (lists SLAs the citizen is responsible for).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MyMandateDto {
    /// The mandate this citizen represents, if they are bound to one.
    pub mandate: Option<MandateDto>,
    /// The verification level recorded on the binding row (`email`/`directory`/`strong`).
    pub binding_level: Option<String>,
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

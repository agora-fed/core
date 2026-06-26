//! # Citizen profile — the social-substrate layer atop credential auth (ADR-0010).
//!
//! This module implements the `GET /me` and `PATCH /me` endpoints' service layer: read and update
//! the authenticated citizen's profile (display name, bio, handle, privacy toggle). Avatar / cover
//! upload routes go through the storage crate (W1.2) and only land their object keys here.
//!
//! Profile mutations NEVER touch credentials — the CPF and password hash live in
//! `auth_credential` and remain inaccessible from this surface, so a profile patch cannot
//! escalate identity. The opposite is also true: a credential operation never modifies the
//! social face of the citizen. That separation is what lets us federate (W2) without leaking
//! sovereign credentials.

use dsoc_api_contract::ProfileDto;
use dsoc_core::ids::{CitizenId, OrgId};
use dsoc_core::{Error, Result};
use dsoc_db::Db;

use crate::domain;
use crate::queries::{self, ProfileRow};

/// Max accepted bio length from the API (DB caps at 1000 as the last-line guard).
const BIO_MAX: usize = 500;
/// Max accepted display-name length.
const DISPLAY_NAME_MAX: usize = 80;
/// Min/max handle length (matches the DB CHECK and Mastodon's de-facto range).
const HANDLE_MIN: usize = 3;
const HANDLE_MAX: usize = 32;

/// The profile service. Holds the database handle and the avatar/cover URL base used to project
/// stored object keys into publicly resolvable URLs (the storage crate owns the actual bytes;
/// this only renders the URL prefix).
#[derive(Clone, Debug)]
pub struct ProfileService {
    db: Db,
    media_base_url: String,
}

impl ProfileService {
    /// Build the service. `media_base_url` is the public origin under which avatar/cover object
    /// keys resolve, e.g. `https://democracia.social.br/media`. Trailing slash is normalized.
    #[must_use]
    pub fn new(db: Db, media_base_url: impl Into<String>) -> Self {
        let raw = media_base_url.into();
        let trimmed = raw.trim_end_matches('/').to_owned();
        Self {
            db,
            media_base_url: trimmed,
        }
    }

    /// Build from `AppState`. The media base comes from `MEDIA_BASE_URL`; until the storage crate
    /// (W1.2) lands, the env var may be unset and avatar/cover URLs render as `None` regardless
    /// of the object key — handlers don't have to know.
    #[must_use]
    pub fn from_state(state: &dsoc_app::AppState) -> Self {
        let base = std::env::var("MEDIA_BASE_URL").unwrap_or_default();
        Self::new(state.db.clone(), base)
    }

    /// Read the caller's profile. The org is taken from the authenticated `CallerId` (never from
    /// the request) so a caller cannot read another tenant's profile by id-guessing.
    ///
    /// # Errors
    /// [`Error::NotFound`] if the citizen row was deleted out from under the session;
    /// [`Error::Storage`] on persistence failure.
    pub async fn get(&self, caller: CitizenId, expected_org: OrgId) -> Result<ProfileDto> {
        let row = queries::find_profile(&self.db, caller.as_uuid())
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| Error::NotFound("citizen not found".to_owned()))?;
        if row.org_id != expected_org.as_uuid() {
            // The session is wired to a different org than the citizen row — this should be
            // impossible under normal operation, but if it happens we treat it as a hard auth
            // failure rather than leak the other org's view.
            return Err(Error::Unauthorized);
        }
        Ok(self.row_to_dto(row))
    }

    /// Patch the caller's profile. Validates lengths and handle format; rejects a handle
    /// collision inside the org with a tagged Conflict (`handle_taken`).
    ///
    /// # Errors
    /// [`Error::Validation`] for a bad field; [`Error::Conflict`] (`"handle_taken"`) if the
    /// handle is in use by another citizen in the same org; [`Error::NotFound`] if the row was
    /// deleted; [`Error::Storage`] on persistence failure.
    pub async fn update(
        &self,
        caller: CitizenId,
        expected_org: OrgId,
        update: ProfileUpdate,
    ) -> Result<ProfileDto> {
        // Re-read first to enforce the org-binding (same protection as in `get`) and to give the
        // PATCH a stable error path when the underlying row has vanished.
        let current = queries::find_profile(&self.db, caller.as_uuid())
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| Error::NotFound("citizen not found".to_owned()))?;
        if current.org_id != expected_org.as_uuid() {
            return Err(Error::Unauthorized);
        }

        let normalized = normalize_update(update)?;
        let row = queries::update_profile(
            &self.db,
            caller.as_uuid(),
            normalized.display_name,
            normalized.bio,
            normalized.handle,
            normalized.is_public,
        )
        .await
        .map_err(map_update_sqlx)?
        .ok_or_else(|| Error::NotFound("citizen not found".to_owned()))?;
        Ok(self.row_to_dto(row))
    }

    fn row_to_dto(&self, row: ProfileRow) -> ProfileDto {
        let citizen = CitizenId::from_uuid(row.citizen_id);
        ProfileDto {
            citizen_id: row.citizen_id,
            org_id: row.org_id,
            handle: row.handle,
            public_handle: domain::public_handle(citizen),
            display_name: row.display_name,
            bio: row.bio,
            avatar_url: self.object_url(row.avatar_object_key.as_deref()),
            cover_url: self.object_url(row.cover_object_key.as_deref()),
            is_public: row.is_public,
            verification_level: row.verification_level,
            created_at: row.created_at,
        }
    }

    fn object_url(&self, key: Option<&str>) -> Option<String> {
        let key = key?;
        if self.media_base_url.is_empty() {
            // Storage backend not configured yet (pre-W1.2). The handler tolerates `None`.
            return None;
        }
        Some(format!("{}/{key}", self.media_base_url))
    }
}

/// Input for [`ProfileService::update`]. Mirrors [`dsoc_api_contract::ProfileUpdateDto`] but uses
/// the two-level Option convention internally: outer `None` = leave alone, `Some(None)` = clear,
/// `Some(Some(v))` = set.
#[derive(Debug, Clone, Default)]
pub struct ProfileUpdate {
    pub display_name: Option<Option<String>>,
    pub bio: Option<Option<String>>,
    pub handle: Option<Option<String>>,
    pub is_public: Option<bool>,
}

impl From<dsoc_api_contract::ProfileUpdateDto> for ProfileUpdate {
    fn from(dto: dsoc_api_contract::ProfileUpdateDto) -> Self {
        // Empty string from the wire = clear (NULL) the column. This is the only way the JSON
        // body can express "remove the bio" given a flat optional field.
        let into_clear_or_set = |v: Option<String>| {
            v.map(|s| if s.is_empty() { None } else { Some(s) })
        };
        Self {
            display_name: into_clear_or_set(dto.display_name),
            bio: into_clear_or_set(dto.bio),
            handle: into_clear_or_set(dto.handle),
            is_public: dto.is_public,
        }
    }
}

fn normalize_update(update: ProfileUpdate) -> Result<ProfileUpdate> {
    let display_name = match update.display_name {
        Some(Some(v)) => {
            let trimmed = v.trim().to_owned();
            if trimmed.is_empty() {
                Some(None)
            } else if trimmed.chars().count() > DISPLAY_NAME_MAX {
                return Err(Error::Validation(format!(
                    "display_name longer than {DISPLAY_NAME_MAX} chars"
                )));
            } else {
                Some(Some(trimmed))
            }
        }
        other => other,
    };

    let bio = match update.bio {
        Some(Some(v)) => {
            let trimmed = v.trim().to_owned();
            if trimmed.is_empty() {
                Some(None)
            } else if trimmed.chars().count() > BIO_MAX {
                return Err(Error::Validation(format!(
                    "bio longer than {BIO_MAX} chars"
                )));
            } else {
                Some(Some(trimmed))
            }
        }
        other => other,
    };

    let handle = match update.handle {
        Some(Some(v)) => {
            let trimmed = v.trim().to_owned();
            if trimmed.is_empty() {
                Some(None)
            } else {
                validate_handle(&trimmed)?;
                Some(Some(trimmed))
            }
        }
        other => other,
    };

    Ok(ProfileUpdate {
        display_name,
        bio,
        handle,
        is_public: update.is_public,
    })
}

/// Validate a handle's format. Matches the DB CHECK: 3–32 chars from `[A-Za-z0-9_.-]`, no `..`.
fn validate_handle(handle: &str) -> Result<()> {
    let len = handle.chars().count();
    if !(HANDLE_MIN..=HANDLE_MAX).contains(&len) {
        return Err(Error::Validation(format!(
            "handle must be {HANDLE_MIN}–{HANDLE_MAX} chars"
        )));
    }
    if handle.contains("..") {
        return Err(Error::Validation("handle cannot contain '..'".to_owned()));
    }
    if !handle
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
    {
        return Err(Error::Validation(
            "handle may only contain letters, digits, '_', '.', '-'".to_owned(),
        ));
    }
    Ok(())
}

fn map_sqlx(error: sqlx::Error) -> Error {
    match error {
        sqlx::Error::RowNotFound => Error::NotFound("citizen not found".to_owned()),
        other => Error::Storage(Box::new(other)),
    }
}

/// Same as `map_sqlx`, but specifically catches the `(org_id, handle)` unique-violation from the
/// citizen profile migration (`citizen_org_handle_unique`) and surfaces it as a tagged Conflict
/// the http layer renders as "Esse nome de usuário já está em uso." — distinct from the bare
/// "Conflito de estado" the user otherwise can't act on.
fn map_update_sqlx(error: sqlx::Error) -> Error {
    if let sqlx::Error::Database(db) = &error {
        if db.is_unique_violation() && db.constraint().unwrap_or("").contains("handle") {
            return Error::Conflict("handle_taken".to_owned());
        }
    }
    map_sqlx(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_format_accepts_normal() {
        assert!(validate_handle("ana").is_ok());
        assert!(validate_handle("ana.lima_99").is_ok());
        assert!(validate_handle("a-b-c").is_ok());
    }

    #[test]
    fn handle_rejects_bad() {
        assert!(validate_handle("ab").is_err()); // too short
        assert!(validate_handle("a".repeat(33).as_str()).is_err()); // too long
        assert!(validate_handle("ana..lima").is_err()); // double dot
        assert!(validate_handle("ana@lima").is_err()); // illegal char
        assert!(validate_handle("ana lima").is_err()); // space
    }

    #[test]
    fn empty_string_clears() {
        let update = ProfileUpdate {
            display_name: Some(Some(String::new())),
            ..Default::default()
        };
        let normalized = normalize_update(update).unwrap();
        assert_eq!(normalized.display_name, Some(None));
    }

    #[test]
    fn whitespace_only_clears() {
        let update = ProfileUpdate {
            bio: Some(Some("   \n  ".to_owned())),
            ..Default::default()
        };
        let normalized = normalize_update(update).unwrap();
        assert_eq!(normalized.bio, Some(None));
    }

    #[test]
    fn long_bio_is_rejected() {
        let update = ProfileUpdate {
            bio: Some(Some("a".repeat(BIO_MAX + 1))),
            ..Default::default()
        };
        assert!(normalize_update(update).is_err());
    }
}

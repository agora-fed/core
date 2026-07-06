//! ActivityPub `Actor` (a `Person`) plus the voter -> candidate -> official identity progression.
//!
//! ADR-0005: a citizen/voter is an AP Actor, and a candidacy/mandate is **the same Actor evolving
//! role** — "voter -> candidate -> official -> governor" is a progression of role bindings on one
//! stable Actor identity, which the scorecard follows. So the derivation here keeps `id`,
//! `preferredUsername`, `inbox`, and `outbox` **stable across role changes**; only the
//! accountability `role` attribute moves. The test `actor_identity_is_preserved_across_roles`
//! asserts exactly this invariant.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use dsoc_api_contract::MandateDto;

use crate::signatures::PublicKey;
use crate::vocab::Context;

/// The AP actor type. Citizens and mandates are modeled as `Person`; the remaining variants exist
/// so an instance/service actor can be expressed without a second model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActorType {
    /// A human-scale identity (citizen, candidate, official). The platform's only actor today.
    Person,
    /// A non-human automated actor (e.g. the instance itself).
    Service,
    /// An application actor.
    Application,
    /// A collective actor.
    Group,
}

/// The role an identity holds in the consequence loop. The progression is monotonic in practice
/// (a voter may become a candidate, then an official, then a governor) but the **identity is one**.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorRole {
    /// A citizen who participates and supports proposals (the entry point; no mandate yet).
    Voter,
    /// A declared candidacy directed at an office.
    Candidate,
    /// A sitting official accountable to the loop.
    Official,
    /// An executive officeholder (e.g. prefeito/governador).
    Governor,
}

impl ActorRole {
    /// Derive the role a `MandateDto` currently expresses. A mandate is always at least a candidacy;
    /// once it is a sitting office (`is_candidate == false`) it is an official. Voter/Governor are
    /// expressed through dedicated derivations (a voter has no mandate; governor is an office label).
    #[must_use]
    pub fn from_mandate(mandate: &MandateDto) -> Self {
        if mandate.is_candidate {
            Self::Candidate
        } else {
            Self::Official
        }
    }
}

/// An ActivityStreams `Image` object, as consumed by Mastodon et al. for an actor's `icon`
/// (avatar) and `image` (header/cover). The `url` MUST be absolute and publicly dereferenceable —
/// remote instances fetch and cache it from their own servers, so a relative path renders blank.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaImage {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(rename = "mediaType", skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    pub url: String,
}

impl MediaImage {
    /// Build an `Image` object from an already-absolute URL.
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            kind: "Image".to_owned(),
            media_type: None,
            url: url.into(),
        }
    }
}

/// An ActivityPub Actor document (`Person`), JSON-LD aware.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Actor {
    /// JSON-LD context (ActivityStreams + security when a key is present).
    #[serde(rename = "@context")]
    pub context: Context,
    /// The actor's stable, public, dereferenceable id (also its self-link). Stable across roles.
    pub id: String,
    /// AP type (`Person`).
    #[serde(rename = "type")]
    pub kind: ActorType,
    /// The WebFinger local part — the stable handle. Never changes when the role changes.
    #[serde(rename = "preferredUsername")]
    pub preferred_username: String,
    /// Optional human display name (may change; not part of the stable identity).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Inbox endpoint (`{id}/inbox`).
    pub inbox: String,
    /// Outbox endpoint (`{id}/outbox`).
    pub outbox: String,
    /// The accountability role this identity currently holds (extension term `dsoc:role`).
    #[serde(rename = "dsoc:role", skip_serializing_if = "Option::is_none")]
    pub role: Option<ActorRole>,
    /// Avatar image (Mastodon `icon`). Absolute URL required.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<MediaImage>,
    /// Header/cover image (Mastodon `image`). Absolute URL required.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<MediaImage>,
    /// HTTP Signatures public key, when published (extension via the security context).
    #[serde(rename = "publicKey", skip_serializing_if = "Option::is_none")]
    pub public_key: Option<PublicKey>,
}

/// Build the canonical, stable actor id URL for `handle` on `host`.
///
/// IPv6 hosts must already be bracketed by the caller (e.g. `[2001:db8::1]`).
#[must_use]
pub fn actor_id(host: &str, handle: &str) -> String {
    format!("https://{host}/actors/{handle}")
}

/// The stable handle for a mandate identity, derived only from its **stable** id so it never moves
/// when office/role/display-name change.
#[must_use]
pub fn mandate_handle(mandate_id: Uuid) -> String {
    format!("mandate-{}", mandate_id.as_simple())
}

/// The stable handle for a citizen/voter identity, derived only from its stable id.
#[must_use]
pub fn citizen_handle(citizen_id: Uuid) -> String {
    format!("citizen-{}", citizen_id.as_simple())
}

impl Actor {
    /// Build a `Person` actor for `handle` on `host` with an optional role and display name.
    #[must_use]
    pub fn person(host: &str, handle: &str, role: Option<ActorRole>, name: Option<String>) -> Self {
        let id = actor_id(host, handle);
        Self {
            context: Context::actor(),
            inbox: format!("{id}/inbox"),
            outbox: format!("{id}/outbox"),
            id,
            kind: ActorType::Person,
            preferred_username: handle.to_owned(),
            name,
            role,
            icon: None,
            image: None,
            public_key: None,
        }
    }

    /// Attach a published HTTP Signatures public key (consumes and returns for a fluent build).
    #[must_use]
    pub fn with_public_key(mut self, key: PublicKey) -> Self {
        self.public_key = Some(key);
        self
    }

    /// Attach avatar/cover images. Each argument is an **absolute** URL; `None` leaves the field
    /// unset (Mastodon then falls back to its default avatar/header).
    #[must_use]
    pub fn with_images(mut self, icon_url: Option<String>, image_url: Option<String>) -> Self {
        self.icon = icon_url.map(MediaImage::new);
        self.image = image_url.map(MediaImage::new);
        self
    }
}

/// Derive the AP Actor for a mandate at its **currently expressed** role.
#[must_use]
pub fn derive_actor(mandate: &MandateDto, host: &str) -> Actor {
    derive_actor_with_role(mandate, host, ActorRole::from_mandate(mandate))
}

/// Derive the AP Actor for a mandate at an **explicit** role. `id`/`preferredUsername`/`inbox`/
/// `outbox` depend only on the mandate's stable id, so they are identical for every role — this is
/// the federated expression of "one identity following the role progression" (ADR-0005).
#[must_use]
pub fn derive_actor_with_role(mandate: &MandateDto, host: &str, role: ActorRole) -> Actor {
    Actor::person(
        host,
        &mandate_handle(mandate.id),
        Some(role),
        Some(mandate.display_name.clone()),
    )
}

/// Derive the AP Actor for a citizen/voter (the entry point of the progression).
#[must_use]
pub fn derive_citizen_actor(citizen_id: Uuid, host: &str) -> Actor {
    Actor::person(
        host,
        &citizen_handle(citizen_id),
        Some(ActorRole::Voter),
        None,
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const HOST: &str = "[2001:db8::1]";

    fn mandate(is_candidate: bool) -> MandateDto {
        MandateDto {
            id: Uuid::nil(),
            office: "vereador".to_owned(),
            display_name: "Fulana de Tal".to_owned(),
            is_candidate,
            onboarded: false,
            party: None,
            uf: None,
            house: None,
            avatar_url: None,
            public_email: None,
            sphere: "federal".to_owned(),
            has_verified_operator: false,
        }
    }

    #[test]
    fn role_is_derived_from_candidacy_flag() {
        assert_eq!(
            ActorRole::from_mandate(&mandate(true)),
            ActorRole::Candidate
        );
        assert_eq!(
            ActorRole::from_mandate(&mandate(false)),
            ActorRole::Official
        );
    }

    #[test]
    fn actor_identity_is_preserved_across_roles() {
        let m = mandate(true);
        let candidate = derive_actor_with_role(&m, HOST, ActorRole::Candidate);
        let official = derive_actor_with_role(&m, HOST, ActorRole::Official);
        let governor = derive_actor_with_role(&m, HOST, ActorRole::Governor);

        // The stable identity coordinates never move when the role changes.
        for other in [&official, &governor] {
            assert_eq!(candidate.id, other.id);
            assert_eq!(candidate.preferred_username, other.preferred_username);
            assert_eq!(candidate.inbox, other.inbox);
            assert_eq!(candidate.outbox, other.outbox);
        }
        // Only the role attribute moves along the progression.
        assert_eq!(candidate.role, Some(ActorRole::Candidate));
        assert_eq!(official.role, Some(ActorRole::Official));
        assert_eq!(governor.role, Some(ActorRole::Governor));
        assert_ne!(candidate.role, official.role);
    }

    #[test]
    fn actor_endpoints_are_derived_from_id() {
        let actor = derive_actor(&mandate(false), HOST);
        assert_eq!(
            actor.id,
            "https://[2001:db8::1]/actors/mandate-00000000000000000000000000000000"
        );
        assert_eq!(actor.inbox, format!("{}/inbox", actor.id));
        assert_eq!(actor.outbox, format!("{}/outbox", actor.id));
        assert_eq!(actor.kind, ActorType::Person);
    }

    #[test]
    fn citizen_actor_is_a_voter() {
        let actor = derive_citizen_actor(Uuid::nil(), HOST);
        assert_eq!(actor.role, Some(ActorRole::Voter));
        assert!(actor.preferred_username.starts_with("citizen-"));
        assert!(actor.name.is_none());
    }

    #[test]
    fn actor_serializes_with_jsonld_and_extension_fields() {
        let actor = derive_actor(&mandate(true), HOST)
            .with_public_key(PublicKey::main_key("https://[2001:db8::1]/actors/x", "PEM"));
        let json = serde_json::to_string(&actor).unwrap();
        assert!(json.contains("\"@context\""));
        assert!(json.contains("\"type\":\"Person\""));
        assert!(json.contains("\"preferredUsername\""));
        assert!(json.contains("\"dsoc:role\":\"candidate\""));
        assert!(json.contains("\"publicKey\""));
        assert!(json.contains("\"publicKeyPem\":\"PEM\""));
    }

    #[test]
    fn actor_round_trips_through_serde() {
        let actor = derive_actor(&mandate(false), HOST);
        let json = serde_json::to_string(&actor).unwrap();
        let back: Actor = serde_json::from_str(&json).unwrap();
        assert_eq!(actor, back);
    }
}

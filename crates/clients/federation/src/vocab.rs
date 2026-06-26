//! JSON-LD vocabulary constants and the `@context` helper used across the AP surface.
//!
//! ActivityPub objects are JSON-LD documents; their meaning is fixed by the `@context`. We extend
//! the standard ActivityStreams context with a small, namespaced **accountability vocabulary**
//! (the way Lemmy/Mobilizon/Mastodon extend AP for their domains, per ADR-0005) so that remote
//! instances can render and relay the consequence loop (proposals, consensus clusters, SLAs,
//! scorecards) without us inventing a closed protocol.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The W3C ActivityStreams 2.0 context — the base vocabulary every AP object declares.
pub const ACTIVITYSTREAMS_NS: &str = "https://www.w3.org/ns/activitystreams";

/// The security vocabulary that defines `publicKey`/`publicKeyPem` (HTTP Signatures keys).
pub const SECURITY_NS: &str = "https://w3id.org/security/v1";

/// IRI namespace for this platform's accountability extension terms.
pub const ACCOUNTABILITY_NS: &str = "https://democracia.social.br/ns/accountability#";

/// JSON-LD prefix bound to [`ACCOUNTABILITY_NS`]; extension types/properties use it (`dsoc:Proposal`).
pub const ACCOUNTABILITY_PREFIX: &str = "dsoc";

/// Software name advertised by NodeInfo and used to identify this implementation in the fediverse.
pub const SOFTWARE_NAME: &str = "democracia-social";

/// Software version advertised by NodeInfo (the crate version, single source of truth).
pub const SOFTWARE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Build the fully-qualified extension term for `name` (e.g. `Proposal` -> `dsoc:Proposal`).
#[must_use]
pub fn term(name: &str) -> String {
    format!("{ACCOUNTABILITY_PREFIX}:{name}")
}

/// A JSON-LD `@context` value. ActivityPub allows `@context` to be a single IRI **or** an array of
/// IRIs and inline term definitions; this transparent wrapper preserves that flexibility while still
/// giving us typed, tested constructors for the three shapes we emit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Context(Value);

impl Context {
    /// The plain ActivityStreams context (for objects/activities that use no extension terms).
    #[must_use]
    pub fn activitystreams() -> Self {
        Self(json!(ACTIVITYSTREAMS_NS))
    }

    /// ActivityStreams + the security context, required by any Actor that publishes a `publicKey`.
    #[must_use]
    pub fn actor() -> Self {
        Self(json!([ACTIVITYSTREAMS_NS, SECURITY_NS]))
    }

    /// ActivityStreams + the inline accountability namespace, for our custom civic objects.
    #[must_use]
    pub fn accountability() -> Self {
        Self(json!([
            ACTIVITYSTREAMS_NS,
            { ACCOUNTABILITY_PREFIX: ACCOUNTABILITY_NS }
        ]))
    }

    /// Borrow the underlying JSON value (read-only; the context is immutable once built).
    #[must_use]
    pub fn as_value(&self) -> &Value {
        &self.0
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn activitystreams_context_is_the_canonical_iri() {
        assert_eq!(
            Context::activitystreams().as_value(),
            &json!(ACTIVITYSTREAMS_NS)
        );
    }

    #[test]
    fn actor_context_includes_security_for_public_keys() {
        let value = Context::actor();
        let arr = value.as_value().as_array().unwrap();
        assert_eq!(arr[0], json!(ACTIVITYSTREAMS_NS));
        assert_eq!(arr[1], json!(SECURITY_NS));
    }

    #[test]
    fn accountability_context_binds_the_dsoc_prefix() {
        let value = Context::accountability();
        let arr = value.as_value().as_array().unwrap();
        assert_eq!(arr[0], json!(ACTIVITYSTREAMS_NS));
        assert_eq!(arr[1][ACCOUNTABILITY_PREFIX], json!(ACCOUNTABILITY_NS));
    }

    #[test]
    fn term_is_prefixed() {
        assert_eq!(term("Proposal"), "dsoc:Proposal");
    }

    #[test]
    fn context_round_trips_through_serde() {
        let ctx = Context::accountability();
        let json_str = serde_json::to_string(&ctx).unwrap();
        let back: Context = serde_json::from_str(&json_str).unwrap();
        assert_eq!(ctx, back);
    }
}

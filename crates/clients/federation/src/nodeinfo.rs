//! NodeInfo 2.1 builders — the fediverse's machine-readable instance descriptor.
//!
//! Two pure builders: [`nodeinfo_discovery`] returns the `.well-known/nodeinfo` link document that
//! points at the schema-2.1 document, and [`nodeinfo_2_1`] returns that document. Both are read-only
//! and take no database — counts are supplied by the caller (the gateway, in Phase 3).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::ToSchema;

use crate::vocab::{ACCOUNTABILITY_NS, SOFTWARE_NAME, SOFTWARE_VERSION};

/// The NodeInfo 2.1 schema relation advertised by the discovery document.
pub const NODEINFO_2_1_REL: &str = "http://nodeinfo.diaspora.software/ns/schema/2.1";

/// The `software` block of a NodeInfo document (name + version of this implementation).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NodeInfoSoftware {
    /// Implementation name (e.g. `democracia-social`).
    pub name: String,
    /// Implementation version (the crate version).
    pub version: String,
}

impl NodeInfoSoftware {
    /// This implementation's software descriptor.
    #[must_use]
    pub fn current() -> Self {
        Self {
            name: SOFTWARE_NAME.to_owned(),
            version: SOFTWARE_VERSION.to_owned(),
        }
    }
}

/// Build the `.well-known/nodeinfo` discovery document linking to the 2.1 schema document on `host`.
#[must_use]
pub fn nodeinfo_discovery(host: &str) -> Value {
    json!({
        "links": [
            {
                "rel": NODEINFO_2_1_REL,
                "href": format!("https://{host}/nodeinfo/2.1"),
            }
        ]
    })
}

/// Build the NodeInfo 2.1 document.
///
/// `user_count` and `open_registrations` are supplied by the caller; this builder reaches no
/// database. `protocols` is fixed to `["activitypub"]` (ADR-0005). The accountability vocabulary
/// IRI is advertised under `metadata` so peers can discover the extension.
#[must_use]
pub fn nodeinfo_2_1(user_count: u64, open_registrations: bool) -> Value {
    let software = serde_json::to_value(NodeInfoSoftware::current()).unwrap_or_else(|_| json!({}));
    json!({
        "version": "2.1",
        "software": software,
        "protocols": ["activitypub"],
        "services": { "inbound": [], "outbound": [] },
        "openRegistrations": open_registrations,
        "usage": { "users": { "total": user_count } },
        "metadata": { "accountabilityVocabulary": ACCOUNTABILITY_NS },
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const HOST: &str = "[2001:db8::1]";

    #[test]
    fn discovery_links_to_the_schema_2_1_document() {
        let doc = nodeinfo_discovery(HOST);
        let link = &doc["links"][0];
        assert_eq!(link["rel"], NODEINFO_2_1_REL);
        assert_eq!(link["href"], "https://[2001:db8::1]/nodeinfo/2.1");
    }

    #[test]
    fn document_advertises_activitypub_and_software() {
        let doc = nodeinfo_2_1(1234, false);
        assert_eq!(doc["version"], "2.1");
        assert_eq!(doc["protocols"][0], "activitypub");
        assert_eq!(doc["software"]["name"], SOFTWARE_NAME);
        assert_eq!(doc["software"]["version"], SOFTWARE_VERSION);
        assert_eq!(doc["openRegistrations"], false);
        assert_eq!(doc["usage"]["users"]["total"], 1234);
    }

    #[test]
    fn document_advertises_the_accountability_vocabulary() {
        let doc = nodeinfo_2_1(0, true);
        assert_eq!(
            doc["metadata"]["accountabilityVocabulary"],
            ACCOUNTABILITY_NS
        );
        assert_eq!(doc["openRegistrations"], true);
    }

    #[test]
    fn software_descriptor_is_the_crate_version() {
        let sw = NodeInfoSoftware::current();
        assert_eq!(sw.name, "democracia-social");
        assert_eq!(sw.version, env!("CARGO_PKG_VERSION"));
    }
}

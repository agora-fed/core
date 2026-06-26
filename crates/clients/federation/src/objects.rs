//! ActivityPub objects/activities and the namespaced accountability vocabulary.
//!
//! Standard AP carries deliberation: a [`Note`] is a comment, a [`Create`] wraps it for delivery.
//! The civic loop is modeled with custom, namespaced objects ([`Proposal`], [`ConsensusCluster`],
//! [`Sla`], [`Scorecard`], [`SupportTally`]) so a remote instance can render and relay the loop
//! (ADR-0005). Crucially, **support is an aggregate count, never a per-actor `Like`** — see
//! [`SupportTally`] and the privacy invariant enforced in `crate::mapping`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use dsoc_api_contract::SlaStatus;

use crate::vocab::{term, Context};

/// A standard ActivityStreams `Note` — the unit of federated deliberation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    /// JSON-LD context.
    #[serde(rename = "@context")]
    pub context: Context,
    /// Stable, dereferenceable id.
    pub id: String,
    /// AP type (`Note`).
    #[serde(rename = "type")]
    pub kind: NoteType,
    /// Optional title/summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The note body.
    pub content: String,
    /// The actor that authored the note, if attributed.
    #[serde(rename = "attributedTo", skip_serializing_if = "Option::is_none")]
    pub attributed_to: Option<String>,
    /// Addressees (actor ids / collections).
    pub to: Vec<String>,
    /// Publication time.
    pub published: DateTime<Utc>,
}

/// The `Note` type tag (its own enum so the wire value is fixed and typed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoteType {
    /// ActivityStreams `Note`.
    Note,
}

/// A `Create` activity wrapping an object for delivery to inboxes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Create {
    /// JSON-LD context.
    #[serde(rename = "@context")]
    pub context: Context,
    /// Stable, dereferenceable id of the activity.
    pub id: String,
    /// AP type (`Create`).
    #[serde(rename = "type")]
    pub kind: CreateType,
    /// The acting actor id.
    pub actor: String,
    /// The embedded object that was created.
    pub object: Value,
    /// Addressees.
    pub to: Vec<String>,
    /// When the activity was produced.
    pub published: DateTime<Utc>,
}

/// The `Create` activity type tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CreateType {
    /// ActivityStreams `Create`.
    Create,
}

impl Create {
    /// Wrap a serializable object in a `Create` activity addressed `to`.
    ///
    /// # Errors
    /// Returns a [`serde_json::Error`] only if the object fails to serialize (never for our types).
    pub fn wrap<T: Serialize>(
        id: impl Into<String>,
        actor: impl Into<String>,
        object: &T,
        to: Vec<String>,
        published: DateTime<Utc>,
    ) -> Result<Self, serde_json::Error> {
        Ok(Self {
            context: Context::activitystreams(),
            id: id.into(),
            kind: CreateType::Create,
            actor: actor.into(),
            object: serde_json::to_value(object)?,
            to,
            published,
        })
    }
}

/// A civic **Proposal** — a `Note`-compatible custom object directed at a mandate actor. Its
/// `type` is multi-valued (`["Note", "dsoc:Proposal"]`) so AP-only instances still render it as a
/// note while accountability-aware instances treat it as a proposal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    /// JSON-LD context (ActivityStreams + accountability).
    #[serde(rename = "@context")]
    pub context: Context,
    /// Stable, public, dereferenceable id.
    pub id: String,
    /// Multi-type tag, `["Note", "dsoc:Proposal"]`.
    #[serde(rename = "type")]
    pub kind: Vec<String>,
    /// Proposal title.
    pub name: String,
    /// Proposal body.
    pub content: String,
    /// The mandate actor this proposal is directed at.
    pub to: Vec<String>,
    /// Aggregate support — a **count**, never a list of supporters (vote privacy / LGPD).
    #[serde(rename = "dsoc:supportCount")]
    pub support_count: u64,
    /// The consensus cluster this proposal was merged into, if any.
    #[serde(rename = "dsoc:cluster", skip_serializing_if = "Option::is_none")]
    pub cluster: Option<String>,
    /// Publication time.
    pub published: DateTime<Utc>,
}

impl Proposal {
    /// The multi-valued type tag every proposal carries.
    #[must_use]
    pub fn type_tag() -> Vec<String> {
        vec!["Note".to_owned(), term("Proposal")]
    }
}

/// An **aggregate** support tally for a proposal. This is the privacy-preserving representation of
/// "support": it federates only a `totalItems` count and the target — it has **no `actor` and no
/// `items`/`Like`**, so individual vote -> citizen linkage is never federated (ADR-0005 hard
/// constraint 1, LGPD).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SupportTally {
    /// JSON-LD context (accountability).
    #[serde(rename = "@context")]
    pub context: Context,
    /// Stable id, `{proposal-id}#support`.
    pub id: String,
    /// Type tag `dsoc:SupportTally`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The proposal this tally is for.
    #[serde(rename = "dsoc:target")]
    pub target: String,
    /// The aggregate count (the only support signal that crosses the wire).
    #[serde(rename = "totalItems")]
    pub total_items: u64,
}

/// A **consensus cluster** — proposals merged because they express the same demand, directed at a
/// mandate. Federates aggregate counts only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConsensusCluster {
    /// JSON-LD context (accountability).
    #[serde(rename = "@context")]
    pub context: Context,
    /// Stable, public id.
    pub id: String,
    /// Type tag `dsoc:ConsensusCluster`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The mandate actor the cluster is directed at.
    #[serde(rename = "dsoc:directedAt")]
    pub directed_at: String,
    /// Number of proposals merged into the cluster (aggregate).
    #[serde(rename = "totalItems")]
    pub total_items: u64,
    /// Aggregate support across the cluster.
    #[serde(rename = "dsoc:supportCount")]
    pub support_count: u64,
}

/// A **consequence SLA** object — the clock a mandate is held to for a clustered demand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sla {
    /// JSON-LD context (accountability).
    #[serde(rename = "@context")]
    pub context: Context,
    /// Stable id.
    pub id: String,
    /// Type tag `dsoc:Sla`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Current state (reuses the frozen contract enum; serializes snake_case).
    #[serde(rename = "dsoc:status")]
    pub status: SlaStatus,
    /// The proposal/cluster this SLA tracks.
    #[serde(rename = "dsoc:proposal")]
    pub proposal: String,
    /// The mandate actor accountable for it.
    #[serde(rename = "dsoc:directedAt")]
    pub directed_at: String,
}

/// A per-mandate **scorecard** — the public accountability summary that follows the identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scorecard {
    /// JSON-LD context (accountability).
    #[serde(rename = "@context")]
    pub context: Context,
    /// Stable id.
    pub id: String,
    /// Type tag `dsoc:Scorecard`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The mandate actor this scorecard belongs to.
    #[serde(rename = "dsoc:mandate")]
    pub mandate: String,
    /// Proposals answered within SLA.
    #[serde(rename = "dsoc:answered")]
    pub answered: u64,
    /// Proposals ignored (public silence).
    #[serde(rename = "dsoc:ignored")]
    pub ignored: u64,
    /// Median response latency in hours, if any responses exist.
    #[serde(
        rename = "dsoc:medianResponseHours",
        skip_serializing_if = "Option::is_none"
    )]
    pub median_response_hours: Option<f64>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn proposal_type_tag_is_note_compatible() {
        assert_eq!(Proposal::type_tag(), vec!["Note", "dsoc:Proposal"]);
    }

    #[test]
    fn create_wraps_a_note_object() {
        let note = Note {
            context: Context::activitystreams(),
            id: "https://[2001:db8::1]/notes/1".to_owned(),
            kind: NoteType::Note,
            name: None,
            content: "Apoio.".to_owned(),
            attributed_to: Some("https://[2001:db8::1]/actors/citizen-1".to_owned()),
            to: vec!["https://[2001:db8::1]/actors/mandate-1".to_owned()],
            published: DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
        };
        let create = Create::wrap(
            "https://[2001:db8::1]/activities/1",
            "https://[2001:db8::1]/actors/citizen-1",
            &note,
            note.to.clone(),
            note.published,
        )
        .unwrap();
        assert_eq!(create.kind, CreateType::Create);
        assert_eq!(create.object["type"], "Note");
        assert_eq!(create.object["content"], "Apoio.");
    }

    #[test]
    fn sla_status_serializes_via_contract_enum() {
        let sla = Sla {
            context: Context::accountability(),
            id: "https://[2001:db8::1]/sla/1".to_owned(),
            kind: term("Sla"),
            status: SlaStatus::Ignored,
            proposal: "https://[2001:db8::1]/proposals/1".to_owned(),
            directed_at: "https://[2001:db8::1]/actors/mandate-1".to_owned(),
        };
        let json = serde_json::to_value(&sla).unwrap();
        assert_eq!(json["dsoc:status"], "ignored");
        assert_eq!(json["type"], "dsoc:Sla");
    }
}

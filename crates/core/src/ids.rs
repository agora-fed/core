//! Strongly-typed identifier newtypes.
//!
//! A bare `Uuid` must never cross a crate boundary: passing a `ProposalId` where a
//! `VoteId` is expected is a compile error, not a runtime bug. All IDs are UUIDv7
//! (time-ordered) so they sort by creation and index well in PostgreSQL.

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Defines a transparent, `Uuid`-backed identifier newtype with a stable string prefix.
macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident, $prefix:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            /// The textual prefix used in logs and public URLs (e.g. `prop_`).
            pub const PREFIX: &'static str = $prefix;

            /// Generate a fresh, time-ordered identifier (UUIDv7).
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            /// Wrap an existing `Uuid` (e.g. read from the database).
            #[must_use]
            pub const fn from_uuid(id: Uuid) -> Self {
                Self(id)
            }

            /// The inner `Uuid`.
            #[must_use]
            pub const fn as_uuid(&self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            /// Generates a fresh identifier (equivalent to [`Self::new`]).
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}{}", $prefix, self.0)
            }
        }
    };
}

define_id!(/// Identity of a participating citizen.
    CitizenId, "ctz_");
define_id!(/// Identity of an organization / tenant.
    OrgId, "org_");
define_id!(/// A registered mandate or candidacy (vereador → Presidência).
    MandateId, "mnd_");
define_id!(/// A proposal — the primary civic artifact.
    ProposalId, "prop_");
define_id!(/// A support vote on a proposal.
    VoteId, "vote_");
define_id!(/// A consensus cluster of near-duplicate proposals.
    ClusterId, "clst_");
define_id!(/// A comment in a deliberation thread.
    CommentId, "cmt_");
define_id!(/// A participation space (process, assembly, …).
    SpaceId, "spc_");
define_id!(/// A consequence-engine SLA instance.
    SlaId, "sla_");
define_id!(/// A public scorecard entry.
    ScorecardId, "scd_");
define_id!(/// A durable event envelope id.
    EventId, "evt_");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_time_ordered_and_prefixed() {
        let a = ProposalId::new();
        let b = ProposalId::new();
        // UUIDv7 is time-ordered: a created before b sorts before b.
        assert!(a <= b);
        assert!(a.to_string().starts_with("prop_"));
    }

    #[test]
    fn ids_roundtrip_through_json_transparently() {
        let id = MandateId::new();
        let json = serde_json::to_string(&id).unwrap();
        // transparent: serializes as the bare uuid string, no wrapper object.
        assert!(json.contains(&id.as_uuid().to_string()));
        let back: MandateId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn distinct_id_types_do_not_unify() {
        // This is a documentation test of intent: ProposalId and VoteId are different
        // types, so the compiler rejects passing one for the other. (Compile-time only.)
        let p = ProposalId::new();
        let v = VoteId::new();
        assert_ne!(p.as_uuid(), v.as_uuid());
    }
}

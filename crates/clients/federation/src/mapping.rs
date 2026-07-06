//! Map the frozen `api-contract` DTOs onto AP objects of the accountability vocabulary.
//!
//! This is where the platform's public shapes become federatable civic objects. The load-bearing
//! rule (ADR-0005 hard constraint 1, LGPD): **support is an aggregate count, never a per-actor
//! public `Like`.** A proposal carries `dsoc:supportCount`, and the standalone support signal is a
//! [`SupportTally`] with `totalItems` only — never an `actor`, never an `items`/`Like` list. The
//! test `support_is_an_aggregate_never_a_per_actor_like` asserts this invariant on the wire bytes.

use uuid::Uuid;

use dsoc_api_contract::{MandateDto, ProposalDto, ScorecardDto, SlaStatus};

use crate::actor::{actor_id, derive_actor, mandate_handle, Actor};
use crate::objects::{ConsensusCluster, Proposal, Scorecard, Sla, SupportTally};
use crate::vocab::{term, Context};

/// Build the canonical, public URL for an object of `segment` with `id` on `host`.
#[must_use]
fn object_url(host: &str, segment: &str, id: Uuid) -> String {
    format!("https://{host}/{segment}/{}", id.as_simple())
}

/// The mandate actor id a proposal/cluster/SLA is directed at, derived from the stable mandate id.
fn mandate_actor_id(host: &str, mandate_id: Uuid) -> String {
    actor_id(host, &mandate_handle(mandate_id))
}

/// Map a [`MandateDto`] to its AP Actor (re-exposes [`derive_actor`] under the mapping surface).
#[must_use]
pub fn mandate_to_actor(mandate: &MandateDto, host: &str) -> Actor {
    derive_actor(mandate, host)
}

/// Map a [`ProposalDto`] to the AP [`Proposal`] object, directed at its mandate actor.
///
/// Support is carried as the aggregate `dsoc:supportCount` — the proposal never lists supporters.
#[must_use]
pub fn proposal_to_ap(proposal: &ProposalDto, host: &str) -> Proposal {
    Proposal {
        context: Context::accountability(),
        id: object_url(host, "proposals", proposal.id),
        kind: Proposal::type_tag(),
        name: proposal.title.clone(),
        content: proposal.body.clone(),
        to: vec![mandate_actor_id(host, proposal.mandate_id)],
        support_count: proposal.support_count,
        cluster: proposal
            .cluster_id
            .map(|cid| object_url(host, "clusters", cid)),
        published: proposal.created_at,
    }
}

/// Build the standalone aggregate [`SupportTally`] for a proposal — the ONLY federated support
/// signal. It exposes a count and a target, and deliberately carries no actor and no item list.
#[must_use]
pub fn support_tally(proposal: &ProposalDto, host: &str) -> SupportTally {
    SupportTally {
        context: Context::accountability(),
        id: format!("{}#support", object_url(host, "proposals", proposal.id)),
        kind: term("SupportTally"),
        target: object_url(host, "proposals", proposal.id),
        total_items: proposal.support_count,
    }
}

/// Map a [`ScorecardDto`] to the AP [`Scorecard`] object that follows the mandate's identity.
#[must_use]
pub fn scorecard_to_ap(scorecard: &ScorecardDto, host: &str) -> Scorecard {
    Scorecard {
        context: Context::accountability(),
        id: format!("{}/scorecard", mandate_actor_id(host, scorecard.mandate_id)),
        kind: term("Scorecard"),
        mandate: mandate_actor_id(host, scorecard.mandate_id),
        answered: scorecard.answered,
        ignored: scorecard.ignored,
        median_response_hours: scorecard.median_response_hours,
    }
}

/// Map an SLA state to the AP [`Sla`] object tracking a proposal directed at a mandate.
#[must_use]
pub fn sla_to_ap(status: SlaStatus, proposal_id: Uuid, mandate_id: Uuid, host: &str) -> Sla {
    Sla {
        context: Context::accountability(),
        id: format!("{}/sla", object_url(host, "proposals", proposal_id)),
        kind: term("Sla"),
        status,
        proposal: object_url(host, "proposals", proposal_id),
        directed_at: mandate_actor_id(host, mandate_id),
    }
}

/// Build the AP [`ConsensusCluster`] object directed at a mandate, with aggregate counts only.
#[must_use]
pub fn cluster_to_ap(
    cluster_id: Uuid,
    mandate_id: Uuid,
    proposal_count: u64,
    support_count: u64,
    host: &str,
) -> ConsensusCluster {
    ConsensusCluster {
        context: Context::accountability(),
        id: object_url(host, "clusters", cluster_id),
        kind: term("ConsensusCluster"),
        directed_at: mandate_actor_id(host, mandate_id),
        total_items: proposal_count,
        support_count,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};

    const HOST: &str = "[2001:db8::1]";

    fn proposal() -> ProposalDto {
        ProposalDto {
            id: Uuid::nil(),
            title: "Recapear a Rua das Flores".to_owned(),
            body: "A via está intransitável.".to_owned(),
            mandate_id: Uuid::nil(),
            cluster_id: Some(Uuid::nil()),
            support_count: 4096,
            threshold: 8192,
            author_handle: None,
            author_public_handle: None,
            author_avatar_url: None,
            created_at: DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
        }
    }

    #[test]
    fn proposal_maps_to_note_compatible_object_directed_at_mandate() {
        let ap = proposal_to_ap(&proposal(), HOST);
        assert_eq!(ap.kind, vec!["Note", "dsoc:Proposal"]);
        assert_eq!(ap.name, "Recapear a Rua das Flores");
        assert_eq!(ap.support_count, 4096);
        assert_eq!(
            ap.to,
            vec!["https://[2001:db8::1]/actors/mandate-00000000000000000000000000000000"]
        );
        assert!(ap.cluster.as_deref().unwrap().contains("/clusters/"));
    }

    /// The privacy invariant (ADR-0005 hard constraint 1 / LGPD): on the wire, support is an
    /// aggregate count and NEVER a per-actor `Like` with an attributed actor or item list.
    #[test]
    fn support_is_an_aggregate_never_a_per_actor_like() {
        let p = proposal();
        let proposal_json = serde_json::to_string(&proposal_to_ap(&p, HOST)).unwrap();
        let tally_json = serde_json::to_string(&support_tally(&p, HOST)).unwrap();

        // The proposal carries the aggregate count...
        assert!(proposal_json.contains("\"dsoc:supportCount\":4096"));
        // ...and the standalone signal carries only an aggregate total.
        assert!(tally_json.contains("\"totalItems\":4096"));
        assert!(tally_json.contains("\"dsoc:SupportTally\""));

        // Neither representation may federate per-actor support. No `Like`, no attributed `actor`,
        // no `items` collection — any of those would leak vote -> citizen linkage.
        for json in [&proposal_json, &tally_json] {
            let lower = json.to_ascii_lowercase();
            assert!(
                !lower.contains("like"),
                "support must not be a Like: {json}"
            );
            assert!(!json.contains("\"actor\""), "no attributed actor: {json}");
            assert!(
                !json.contains("\"items\""),
                "no per-actor item list: {json}"
            );
            assert!(
                !json.contains("attributedTo"),
                "support carries no author: {json}"
            );
        }
    }

    #[test]
    fn scorecard_follows_the_mandate_actor_identity() {
        let sc = ScorecardDto {
            mandate_id: Uuid::nil(),
            answered: 7,
            ignored: 2,
            median_response_hours: Some(18.5),
        };
        let ap = scorecard_to_ap(&sc, HOST);
        assert!(ap.mandate.contains("/actors/mandate-"));
        assert_eq!(ap.answered, 7);
        assert_eq!(ap.ignored, 2);
        assert_eq!(ap.median_response_hours, Some(18.5));
    }

    #[test]
    fn sla_maps_status_and_links() {
        let ap = sla_to_ap(SlaStatus::Ignored, Uuid::nil(), Uuid::nil(), HOST);
        assert_eq!(ap.status, SlaStatus::Ignored);
        assert!(ap.proposal.contains("/proposals/"));
        assert!(ap.directed_at.contains("/actors/mandate-"));
    }

    #[test]
    fn cluster_carries_only_aggregate_counts() {
        let ap = cluster_to_ap(Uuid::nil(), Uuid::nil(), 12, 8000, HOST);
        let json = serde_json::to_string(&ap).unwrap();
        assert_eq!(ap.total_items, 12);
        assert_eq!(ap.support_count, 8000);
        assert!(!json.to_ascii_lowercase().contains("like"));
        assert!(!json.contains("\"actor\""));
    }

    #[test]
    fn mandate_maps_to_actor() {
        let m = MandateDto {
            id: Uuid::nil(),
            office: "vereador".to_owned(),
            display_name: "Fulana".to_owned(),
            is_candidate: false,
            onboarded: true,
            party: None,
            uf: None,
            house: None,
            avatar_url: None,
            public_email: None,
            sphere: "federal".to_owned(),
            has_verified_operator: false,
        };
        let actor = mandate_to_actor(&m, HOST);
        assert!(actor.id.contains("/actors/mandate-"));
    }
}

//! Integration tests for `proposals` against a real PostgreSQL (TESTING.md: no mocked database).
//!
//! Each test isolates its data with a unique `org_id`/`mandate_id` so runs never collide, uses a
//! deterministic [`FixedClock`] (never sleeps), and asserts against the `events_log` rows the service
//! wrote through the **transactional outbox** (ADR-0006) — proving the domain change and the event
//! commit atomically rather than trusting a post-commit publish.

// Test code: a failed `expect`/`panic` is a test failure, which is the desired behaviour.
#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use dsoc_core::clock::Clock;
use dsoc_core::events::{Event, EventEnvelope};
use dsoc_core::ids::{CitizenId, ClusterId, EventId, MandateId, OrgId, ProposalId, VoteId};
use dsoc_proposals::{Consumed, NewProposal, NewRevision, ProposalService};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

/// A deterministic clock: time is injected, never read ambiently (TESTING.md).
#[derive(Debug, Clone, Copy)]
struct FixedClock(DateTime<Utc>);

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

fn fixed_clock() -> Arc<dyn Clock> {
    Arc::new(FixedClock(
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
    ))
}

async fn pool() -> PgPool {
    let url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for integration tests");
    PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("connect to PostgreSQL")
}

fn service(db: PgPool) -> ProposalService {
    ProposalService::new(db, fixed_clock())
}

/// Seed an org (FK target for proposal.org_id and events_log.org_id) and return its id.
async fn seed_org(db: &PgPool) -> OrgId {
    let org = OrgId::new();
    sqlx::query("INSERT INTO org (id, slug, name, created_at) VALUES ($1, $2, $3, now())")
        .bind(org.as_uuid())
        .bind(format!("org-{}", org.as_uuid()))
        .bind("Município de Teste")
        .execute(db)
        .await
        .expect("seed org");
    org
}

/// Seed a mandate (FK target for proposal.mandate_id) within `org`.
async fn seed_mandate(db: &PgPool, org: OrgId) -> MandateId {
    let mandate = MandateId::new();
    sqlx::query(
        "INSERT INTO mandate (id, org_id, office, display_name, public_email, created_at) \
         VALUES ($1, $2, 'vereador', 'Fulana de Tal', 'fulana@example.org', now())",
    )
    .bind(mandate.as_uuid())
    .bind(org.as_uuid())
    .execute(db)
    .await
    .expect("seed mandate");
    mandate
}

/// Seed a citizen (FK target for proposal_revision.edited_by) within `org`.
async fn seed_citizen(db: &PgPool, org: OrgId) -> CitizenId {
    let citizen = CitizenId::new();
    sqlx::query(
        "INSERT INTO citizen (id, org_id, oidc_subject, verification_level, created_at) \
         VALUES ($1, $2, $3, 'email', now())",
    )
    .bind(citizen.as_uuid())
    .bind(org.as_uuid())
    .bind(format!("sub-{}", citizen.as_uuid()))
    .execute(db)
    .await
    .expect("seed citizen");
    citizen
}

/// The event types the service wrote to the outbox for `org`, ordered by the time-ordered event id.
async fn outbox_event_types(db: &PgPool, org: OrgId) -> Vec<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT event_type FROM events_log WHERE org_id = $1 ORDER BY id",
    )
    .bind(org.as_uuid())
    .fetch_all(db)
    .await
    .expect("read outbox events")
}

/// The decoded payloads the service wrote to the outbox for `org`, ordered by event id.
async fn outbox_events(db: &PgPool, org: OrgId) -> Vec<Event> {
    let rows = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT payload FROM events_log WHERE org_id = $1 ORDER BY id",
    )
    .bind(org.as_uuid())
    .fetch_all(db)
    .await
    .expect("read outbox payloads");
    rows.into_iter()
        .map(|v| serde_json::from_value(v).expect("decode Event"))
        .collect()
}

fn proposal_text() -> NewProposal {
    NewProposal::validate(
        "Mais creches no centro",
        "Queremos creches em tempo integral",
        100,
    )
    .expect("valid proposal")
}

fn merged_envelope(org: OrgId, proposal: ProposalId, cluster: ClusterId) -> EventEnvelope {
    EventEnvelope {
        id: EventId::new(),
        org,
        at: fixed_clock().now(),
        event: Event::ConsensusProposalMerged { proposal, cluster },
    }
}

fn tally_envelope(org: OrgId, proposal: ProposalId, support_count: u64) -> EventEnvelope {
    EventEnvelope {
        id: EventId::new(),
        org,
        at: fixed_clock().now(),
        event: Event::VoteTallyUpdated {
            proposal,
            support_count,
        },
    }
}

#[tokio::test]
async fn create_persists_and_emits_proposal_created() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;

    let row = svc.create(org, mandate, None, &proposal_text())
        .await
        .expect("create proposal");

    assert_eq!(row.org_id, org.as_uuid());
    assert_eq!(row.mandate_id, mandate.as_uuid());
    assert_eq!(row.status, "draft");
    assert_eq!(row.support_count, 0);
    assert!(row.cluster_id.is_none());
    assert!(row.threshold_crossed_at.is_none());

    // The event was written to the outbox in the same transaction as the insert.
    let events = outbox_events(&db, org).await;
    assert_eq!(events.len(), 1);
    match events[0] {
        Event::ProposalCreated {
            proposal,
            mandate: m,
        } => {
            assert_eq!(proposal, ProposalId::from_uuid(row.id));
            assert_eq!(m, mandate);
        }
        ref other => panic!("expected ProposalCreated, got {other:?}"),
    }
}

#[tokio::test]
async fn cluster_merge_event_links_and_sets_clustered() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;

    let row = svc.create(org, mandate, None, &proposal_text())
        .await
        .expect("create");
    let proposal = ProposalId::from_uuid(row.id);
    let cluster = ClusterId::new();

    let consumed = svc
        .consume(&merged_envelope(org, proposal, cluster))
        .await
        .expect("consume merge");
    assert_eq!(
        consumed,
        Consumed::Linked {
            proposal,
            cluster,
            crossed: false
        }
    );

    let stored = svc.get(proposal).await.expect("get");
    assert_eq!(stored.cluster_id, Some(cluster.as_uuid()));
    assert_eq!(stored.status, "clustered");

    // Linking with no support over threshold emits nothing beyond the create event.
    assert_eq!(
        outbox_event_types(&db, org).await,
        vec!["proposals.created".to_string()]
    );
}

#[tokio::test]
async fn duplicate_cluster_link_is_idempotent() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    let row = svc.create(org, mandate, None, &proposal_text())
        .await
        .expect("create");
    let proposal = ProposalId::from_uuid(row.id);
    let cluster = ClusterId::new();

    svc.consume(&merged_envelope(org, proposal, cluster))
        .await
        .expect("first link");
    // A second, different cluster delivery must NOT relink (guarded by cluster_id IS NULL).
    let other = ClusterId::new();
    let consumed = svc
        .consume(&merged_envelope(org, proposal, other))
        .await
        .expect("second link");
    assert_eq!(
        consumed,
        Consumed::Linked {
            proposal,
            cluster, // still the first cluster
            crossed: false
        }
    );
    let stored = svc.get(proposal).await.expect("get");
    assert_eq!(stored.cluster_id, Some(cluster.as_uuid()));
}

#[tokio::test]
async fn clustered_proposal_crosses_threshold_exactly_once_with_cluster_id() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;

    // threshold = 3 for a compact test.
    let np = NewProposal::validate("Iluminação na praça", "Trocar postes", 3).expect("valid");
    let row = svc.create(org, mandate, None, &np).await.expect("create");
    let proposal = ProposalId::from_uuid(row.id);
    let cluster = ClusterId::new();

    // Link to a cluster first (required for crossing).
    svc.consume(&merged_envelope(org, proposal, cluster))
        .await
        .expect("link");

    // Below threshold: no crossing.
    let c1 = svc
        .consume(&tally_envelope(org, proposal, 2))
        .await
        .expect("tally 2");
    assert_eq!(
        c1,
        Consumed::TallyUpdated {
            proposal,
            support_count: 2,
            crossed: false
        }
    );

    // At threshold while clustered: crossing fires.
    let c2 = svc
        .consume(&tally_envelope(org, proposal, 3))
        .await
        .expect("tally 3");
    assert_eq!(
        c2,
        Consumed::TallyUpdated {
            proposal,
            support_count: 3,
            crossed: true
        }
    );

    // Duplicate / higher tallies must NOT re-fire the crossing (idempotency guard).
    let c3 = svc
        .consume(&tally_envelope(org, proposal, 3))
        .await
        .expect("tally 3 again");
    assert!(matches!(c3, Consumed::TallyUpdated { crossed: false, .. }));
    let c4 = svc
        .consume(&tally_envelope(org, proposal, 50))
        .await
        .expect("tally 50");
    assert!(matches!(c4, Consumed::TallyUpdated { crossed: false, .. }));

    // Exactly one threshold.crossed in the outbox, carrying the cluster id and the mandate.
    let events = outbox_events(&db, org).await;
    let crossings: Vec<&Event> = events
        .iter()
        .filter(|e| matches!(e, Event::ProposalThresholdCrossed { .. }))
        .collect();
    assert_eq!(crossings.len(), 1, "the crossing fires exactly once");
    match crossings[0] {
        Event::ProposalThresholdCrossed {
            proposal: p,
            cluster: c,
            mandate: m,
        } => {
            assert_eq!(*p, proposal);
            assert_eq!(*c, cluster, "the one signal carries the ClusterId");
            assert_eq!(*m, mandate);
        }
        other => panic!("expected ProposalThresholdCrossed, got {other:?}"),
    }

    // The proposal row records the crossing timestamp (the 'already fired' guard).
    let stored = svc.get(proposal).await.expect("get");
    assert!(stored.threshold_crossed_at.is_some());
    assert_eq!(stored.support_count, 50);
}

#[tokio::test]
async fn redelivered_event_id_is_deduped_by_the_consumed_ledger() {
    // ADR-0007 consumer idempotency: re-delivering the SAME originating event id is a no-op even
    // when the (buggy/at-least-once) duplicate carries a higher count the monotonic fold would
    // otherwise apply — proving the event-id ledger, not just the state guard, blocks it.
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    let row = svc.create(org, mandate, None, &proposal_text())
        .await
        .expect("create");
    let proposal = ProposalId::from_uuid(row.id);

    // First delivery of event E folds support to 2.
    let event = EventId::new();
    let first = EventEnvelope {
        id: event,
        org,
        at: fixed_clock().now(),
        event: Event::VoteTallyUpdated {
            proposal,
            support_count: 2,
        },
    };
    let c1 = svc.consume(&first).await.expect("first tally");
    assert_eq!(
        c1,
        Consumed::TallyUpdated {
            proposal,
            support_count: 2,
            crossed: false
        }
    );

    // Re-delivery of the SAME event id, even carrying a higher count, is deduped: support stays 2.
    let duplicate = EventEnvelope {
        id: event,
        org,
        at: fixed_clock().now(),
        event: Event::VoteTallyUpdated {
            proposal,
            support_count: 5,
        },
    };
    let c2 = svc.consume(&duplicate).await.expect("duplicate tally");
    assert_eq!(
        c2,
        Consumed::TallyUpdated {
            proposal,
            support_count: 2,
            crossed: false
        },
        "the consumed ledger blocks the re-delivered event id"
    );

    let stored = svc.get(proposal).await.expect("get");
    assert_eq!(
        stored.support_count, 2,
        "the duplicate's higher count never applied"
    );

    // The ledger recorded the claim exactly once for this consumer.
    let claims: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events_consumed WHERE consumer = 'proposals' AND event_id = $1",
    )
    .bind(event.as_uuid())
    .fetch_one(&db)
    .await
    .expect("count claims");
    assert_eq!(claims, 1);
}

#[tokio::test]
async fn unclustered_proposal_cannot_cross_even_over_threshold() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;

    let np = NewProposal::validate("Sem cluster", "Proposta isolada", 3).expect("valid");
    let row = svc.create(org, mandate, None, &np).await.expect("create");
    let proposal = ProposalId::from_uuid(row.id);

    // Pile on support WAY past the threshold, but the proposal has no cluster.
    let consumed = svc
        .consume(&tally_envelope(org, proposal, 10_000))
        .await
        .expect("tally");
    assert_eq!(
        consumed,
        Consumed::TallyUpdated {
            proposal,
            support_count: 10_000,
            crossed: false
        }
    );

    let stored = svc.get(proposal).await.expect("get");
    assert!(
        stored.threshold_crossed_at.is_none(),
        "no cluster, no crossing"
    );
    // Only the create event exists; the noise never became one accountability signal.
    assert_eq!(
        outbox_event_types(&db, org).await,
        vec!["proposals.created".to_string()]
    );
}

#[tokio::test]
async fn crossing_also_fires_when_cluster_link_arrives_after_support() {
    // Support reaches threshold first (but cannot cross while unclustered); the later cluster link
    // is what folds it into one signal and fires the crossing.
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    let np = NewProposal::validate("Ordem inversa", "Apoio antes do cluster", 5).expect("valid");
    let row = svc.create(org, mandate, None, &np).await.expect("create");
    let proposal = ProposalId::from_uuid(row.id);

    // Support over threshold, still unclustered => no crossing yet.
    let pre = svc
        .consume(&tally_envelope(org, proposal, 9))
        .await
        .expect("tally");
    assert!(matches!(pre, Consumed::TallyUpdated { crossed: false, .. }));

    // Now the cluster link arrives => crossing fires on link.
    let cluster = ClusterId::new();
    let linked = svc
        .consume(&merged_envelope(org, proposal, cluster))
        .await
        .expect("link");
    assert_eq!(
        linked,
        Consumed::Linked {
            proposal,
            cluster,
            crossed: true
        }
    );

    let crossings = outbox_events(&db, org)
        .await
        .into_iter()
        .filter(|e| matches!(e, Event::ProposalThresholdCrossed { .. }))
        .count();
    assert_eq!(crossings, 1);
}

#[tokio::test]
async fn moderation_cleared_publishes_and_emits_published_once() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    let row = svc.create(org, mandate, None, &proposal_text())
        .await
        .expect("create");
    let proposal = ProposalId::from_uuid(row.id);

    let cleared = EventEnvelope {
        id: EventId::new(),
        org,
        at: fixed_clock().now(),
        event: Event::ModerationCleared { proposal },
    };
    let c = svc.consume(&cleared).await.expect("publish");
    assert_eq!(
        c,
        Consumed::Published {
            proposal,
            emitted: true
        }
    );

    let stored = svc.get(proposal).await.expect("get");
    assert_eq!(stored.status, "published");
    assert!(stored.published_at.is_some());

    // A duplicate clearance does not re-publish or re-emit.
    let again = svc.consume(&cleared).await.expect("publish again");
    assert_eq!(
        again,
        Consumed::Published {
            proposal,
            emitted: false
        }
    );

    let published = outbox_event_types(&db, org)
        .await
        .into_iter()
        .filter(|t| t == "proposals.published")
        .count();
    assert_eq!(published, 1, "published emitted exactly once");
}

#[tokio::test]
async fn revisions_are_append_only_and_preserve_history() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    let citizen = seed_citizen(&db, org).await;
    let row = svc.create(org, mandate, None, &proposal_text())
        .await
        .expect("create");
    let proposal = ProposalId::from_uuid(row.id);

    let r1 = NewRevision::validate("Título v2", "Corpo v2").expect("valid");
    svc.add_revision(proposal, citizen, &r1)
        .await
        .expect("rev1");
    let r2 = NewRevision::validate("Título v3", "Corpo v3").expect("valid");
    svc.add_revision(proposal, citizen, &r2)
        .await
        .expect("rev2");

    // History preserved: both revisions remain, oldest-first.
    let revisions = svc.list_revisions(proposal, None, 20).await.expect("list");
    assert_eq!(revisions.len(), 2);
    assert_eq!(revisions[0].title, "Título v2");
    assert_eq!(revisions[1].title, "Título v3");

    // The proposal mirrors the latest content.
    let stored = svc.get(proposal).await.expect("get");
    assert_eq!(stored.title, "Título v3");
    assert_eq!(stored.body, "Corpo v3");

    // Direct count guards against any in-place overwrite (true append-only).
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM proposal_revision WHERE proposal_id = $1")
            .bind(proposal.as_uuid())
            .fetch_one(&db)
            .await
            .expect("count revisions");
    assert_eq!(count, 2);
}

#[tokio::test]
async fn revision_on_missing_proposal_is_not_found() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let citizen = seed_citizen(&db, org).await;
    let r = NewRevision::validate("x", "y").expect("valid");
    let err = svc
        .add_revision(ProposalId::new(), citizen, &r)
        .await
        .expect_err("missing proposal");
    assert_eq!(err.code(), "not_found");
}

#[tokio::test]
async fn tally_for_unknown_proposal_is_ignored() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    // No proposal created for this id.
    let consumed = svc
        .consume(&tally_envelope(org, ProposalId::new(), 5))
        .await
        .expect("consume");
    assert_eq!(consumed, Consumed::Ignored);
    assert!(outbox_event_types(&db, org).await.is_empty());
}

#[tokio::test]
async fn unrelated_event_is_ignored() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let env = EventEnvelope {
        id: EventId::new(),
        org,
        at: fixed_clock().now(),
        event: Event::VoteCast {
            vote: VoteId::new(),
            proposal: ProposalId::new(),
        },
    };
    assert_eq!(svc.consume(&env).await.expect("consume"), Consumed::Ignored);
    assert!(outbox_event_types(&db, org).await.is_empty());
}

#[tokio::test]
async fn cluster_formed_event_is_recognised_but_not_actionable() {
    // consensus.cluster.formed carries no proposal id, so proposals cannot link from it alone.
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let env = EventEnvelope {
        id: EventId::new(),
        org,
        at: fixed_clock().now(),
        event: Event::ConsensusClusterFormed {
            cluster: ClusterId::new(),
            size: 1,
        },
    };
    assert_eq!(svc.consume(&env).await.expect("consume"), Consumed::Ignored);
}

#[tokio::test]
async fn list_paginates_by_keyset() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;

    let a = svc.create(org, mandate, None, &proposal_text()).await.expect("a");
    let b = svc.create(org, mandate, None, &proposal_text()).await.expect("b");

    let (page, total) = svc.list(org, None, 20).await.expect("list");
    assert_eq!(total, 2);
    assert_eq!(page.len(), 2);
    // UUIDv7 ids are time-ordered, so `a` sorts before `b`.
    assert_eq!(page[0].id, a.id);
    assert_eq!(page[1].id, b.id);

    let (after_a, _) = svc
        .list(org, Some(ProposalId::from_uuid(a.id)), 20)
        .await
        .expect("keyset");
    assert_eq!(after_a.len(), 1);
    assert_eq!(after_a[0].id, b.id);
}

#[tokio::test]
async fn create_with_unknown_mandate_conflicts() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    // mandate_id not seeded => foreign-key violation mapped to Conflict.
    let err = svc.create(org, MandateId::new(), None, &proposal_text())
        .await
        .expect_err("unknown mandate");
    assert_eq!(err.code(), "conflict");
    assert!(outbox_event_types(&db, org).await.is_empty());
}

#[tokio::test]
async fn missing_proposal_is_not_found() {
    let db = pool().await;
    let svc = service(db.clone());
    let err = svc.get(ProposalId::new()).await.expect_err("absent");
    assert_eq!(err.code(), "not_found");
}

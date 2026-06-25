//! Integration tests for `consensus` against a real PostgreSQL (TESTING.md: no mocked database).
//!
//! Each test isolates its data with a unique `org_id` so runs never collide, uses a deterministic
//! [`FixedClock`] (never sleeps) and the deterministic [`StubEmbedder`]. Events are emitted through
//! the transactional outbox (ADR-0006), so tests assert against the `events_log` rows the service
//! wrote inside the same transaction as the domain change — proving atomicity rather than trusting a
//! post-commit publish.

// Test code: a failed `expect`/`panic` is a test failure, which is the desired behaviour.
#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use dsoc_consensus::{ClusterService, StubEmbedder, DEFAULT_THRESHOLD};
use dsoc_core::clock::Clock;
use dsoc_core::events::Event;
use dsoc_core::ids::{OrgId, ProposalId};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use uuid::Uuid;

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

/// Seed an org row (FK target for `consensus_cluster.org_id` and `events_log.org_id`) and return its
/// id.
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

fn service(db: PgPool) -> ClusterService {
    ClusterService::new(db, fixed_clock(), Arc::new(StubEmbedder), DEFAULT_THRESHOLD)
}

/// Read the event types the service wrote to the outbox for `org`, ordered by the (time-ordered)
/// event id so the sequence is deterministic even under a fixed clock.
async fn outbox_event_types(db: &PgPool, org: OrgId) -> Vec<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT event_type FROM events_log WHERE org_id = $1 ORDER BY id",
    )
    .bind(org.as_uuid())
    .fetch_all(db)
    .await
    .expect("read outbox events")
}

#[tokio::test]
async fn first_proposal_forms_a_cluster_and_emits_formed() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;

    let outcome = svc
        .ingest(
            org,
            ProposalId::new(),
            "creche em tempo integral no bairro centro",
        )
        .await
        .expect("ingest first proposal");

    assert!(
        matches!(outcome, dsoc_consensus::Placement::Formed { .. }),
        "the first proposal must seed a new cluster"
    );

    // The event was written to the outbox inside the same transaction as the write.
    assert_eq!(
        outbox_event_types(&db, org).await,
        vec!["consensus.cluster.formed".to_string()]
    );
}

#[tokio::test]
async fn near_identical_texts_cluster_together_and_emit_merged() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;

    let first = svc
        .ingest(
            org,
            ProposalId::new(),
            "queremos mais creches no bairro centro da cidade",
        )
        .await
        .expect("ingest first");
    let formed_cluster = match first {
        dsoc_consensus::Placement::Formed { cluster, .. } => cluster,
        other => panic!("expected Formed, got {other:?}"),
    };

    let second = svc
        .ingest(
            org,
            ProposalId::new(),
            "queremos mais creches no bairro centro da cidade agora",
        )
        .await
        .expect("ingest near-duplicate");

    match second {
        dsoc_consensus::Placement::Merged { cluster, size } => {
            assert_eq!(cluster, formed_cluster, "must merge into the first cluster");
            assert_eq!(size, 2, "cluster now has two members");
        }
        other => panic!("expected Merged, got {other:?}"),
    }

    assert_eq!(
        outbox_event_types(&db, org).await,
        vec![
            "consensus.cluster.formed".to_string(),
            "consensus.proposal.merged".to_string(),
        ]
    );
}

#[tokio::test]
async fn dissimilar_texts_form_separate_clusters() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;

    let a = svc
        .ingest(
            org,
            ProposalId::new(),
            "creche em tempo integral no bairro centro",
        )
        .await
        .expect("ingest a");
    let b = svc
        .ingest(
            org,
            ProposalId::new(),
            "reforma completa da iluminação na avenida do litoral",
        )
        .await
        .expect("ingest b");

    let (ca, cb) = match (a, b) {
        (
            dsoc_consensus::Placement::Formed { cluster: ca, .. },
            dsoc_consensus::Placement::Formed { cluster: cb, .. },
        ) => (ca, cb),
        other => panic!("expected two Formed placements, got {other:?}"),
    };
    assert_ne!(ca, cb, "distinct civic asks must not merge");

    let formed = outbox_event_types(&db, org)
        .await
        .into_iter()
        .filter(|t| t == "consensus.cluster.formed")
        .count();
    assert_eq!(formed, 2);
}

#[tokio::test]
async fn duplicate_proposal_ingest_conflicts_without_suppressing_the_event() {
    // Accountability regression guard (ADR-0006): the first ingest commits the placement AND its
    // event atomically via the outbox. A retry hits the `proposal_id` UNIQUE constraint and
    // conflicts — but because the event was committed in the same transaction as the first write, it
    // is neither lost nor duplicated. Exactly one signal remains.
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;

    let proposal = ProposalId::new();
    svc.ingest(org, proposal, "transparência no orçamento municipal")
        .await
        .expect("first ingest");

    let err = svc
        .ingest(org, proposal, "transparência no orçamento municipal")
        .await
        .expect_err("re-ingesting the same proposal must conflict");
    assert_eq!(err.code(), "conflict");

    assert_eq!(
        outbox_event_types(&db, org).await,
        vec!["consensus.cluster.formed".to_string()],
        "the formed event must survive exactly once across the conflicting retry"
    );
}

#[tokio::test]
async fn empty_text_is_rejected_as_validation() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;

    let err = svc
        .ingest(org, ProposalId::new(), "   ")
        .await
        .expect_err("blank text must be rejected");
    assert_eq!(err.code(), "invalid_input");
    assert!(
        outbox_event_types(&db, org).await.is_empty(),
        "no event on validation failure"
    );
}

#[tokio::test]
async fn threshold_boundary_is_respected() {
    // A custom service whose threshold is below the near-duplicate distance forces the second,
    // similar proposal to form its OWN cluster instead of merging — proving the threshold gates the
    // decision deterministically (the stub embedder is deterministic).
    let db = pool().await;
    let strict = ClusterService::new(
        db.clone(),
        fixed_clock(),
        Arc::new(StubEmbedder),
        0.0, // only an exact-direction match may merge
    );
    let org = seed_org(&db).await;

    let first = strict
        .ingest(org, ProposalId::new(), "creches no bairro centro da cidade")
        .await
        .expect("ingest first");
    let second = strict
        .ingest(
            org,
            ProposalId::new(),
            "creches no bairro centro da cidade hoje",
        )
        .await
        .expect("ingest similar");

    assert!(matches!(first, dsoc_consensus::Placement::Formed { .. }));
    assert!(
        matches!(second, dsoc_consensus::Placement::Formed { .. }),
        "below-threshold similarity must not merge under a strict threshold"
    );

    // Two independent clusters formed, so two formed events landed in the outbox.
    assert_eq!(
        outbox_event_types(&db, org).await,
        vec![
            "consensus.cluster.formed".to_string(),
            "consensus.cluster.formed".to_string(),
        ]
    );
}

#[tokio::test]
async fn consume_proposals_created_clusters_the_proposal() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;

    let envelope = dsoc_core::events::EventEnvelope {
        id: dsoc_core::ids::EventId::new(),
        org,
        at: fixed_clock().now(),
        event: Event::ProposalCreated {
            proposal: ProposalId::new(),
            mandate: dsoc_core::ids::MandateId::new(),
        },
    };

    let placement = svc
        .consume(&envelope, "mais creches no centro")
        .await
        .expect("consume should succeed")
        .expect("proposals.created yields a placement");
    assert!(matches!(
        placement,
        dsoc_consensus::Placement::Formed { .. }
    ));

    // Consuming the inbound event drives a placement, which emits its own outbox event.
    assert_eq!(
        outbox_event_types(&db, org).await,
        vec!["consensus.cluster.formed".to_string()]
    );
}

#[tokio::test]
async fn consume_ignores_unrelated_events() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;

    let envelope = dsoc_core::events::EventEnvelope {
        id: dsoc_core::ids::EventId::new(),
        org,
        at: fixed_clock().now(),
        event: Event::VoteCast {
            vote: dsoc_core::ids::VoteId::new(),
            proposal: ProposalId::new(),
        },
    };

    let placement = svc
        .consume(&envelope, "irrelevant")
        .await
        .expect("consume should succeed");
    assert!(placement.is_none(), "non-proposal events are ignored");
    assert!(
        outbox_event_types(&db, org).await.is_empty(),
        "ignored events produce no placement and no outbox emission"
    );
}

#[tokio::test]
async fn get_and_list_clusters_round_trip() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;

    let formed = svc
        .ingest(
            org,
            ProposalId::new(),
            "saneamento básico no distrito norte",
        )
        .await
        .expect("ingest");
    let cluster_id = match formed {
        dsoc_consensus::Placement::Formed { cluster, .. } => cluster,
        other => panic!("expected Formed, got {other:?}"),
    };

    let row = svc.cluster(cluster_id).await.expect("get cluster");
    assert_eq!(row.id, cluster_id.as_uuid());
    assert_eq!(row.org_id, org.as_uuid());
    assert_eq!(row.size, 1);

    let (rows, total) = svc.list_clusters(org, None, 20).await.expect("list");
    assert_eq!(total, 1);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, cluster_id.as_uuid());

    // Keyset cursor past the only row returns an empty page.
    let (empty, _) = svc
        .list_clusters(org, Some(cluster_id), 20)
        .await
        .expect("keyset list");
    assert!(empty.is_empty());
}

#[tokio::test]
async fn missing_cluster_is_not_found() {
    let db = pool().await;
    let svc = service(db.clone());

    let err = svc
        .cluster(dsoc_core::ids::ClusterId::from_uuid(Uuid::now_v7()))
        .await
        .expect_err("absent cluster must be NotFound");
    assert_eq!(err.code(), "not_found");
}

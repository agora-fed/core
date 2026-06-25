//! Integration tests for `dsoc-comments` against a real PostgreSQL (TESTING.md: no mocked
//! database). Each test isolates its data with a fresh `org_id`/`citizen_id` so runs never
//! collide, uses a deterministic [`FixedClock`] (never sleeps), and asserts emissions
//! against the `events_log` rows the service wrote inside the same transaction as the
//! domain change — proving the transactional outbox (ADR-0006) rather than trusting a
//! post-commit publish.

// Test code: a failed `expect`/`panic` is a test failure, which is the desired behaviour.
#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use dsoc_comments::domain::{CommentStatus, VoteWeight, MAX_THREAD_DEPTH};
use dsoc_comments::events::handle_event;
use dsoc_comments::service::CommentService;
use dsoc_core::clock::Clock;
use dsoc_core::events::{Event, EventEnvelope};
use dsoc_core::ids::{CitizenId, CommentId, EventId, OrgId, ProposalId, VoteId};
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
        Utc.with_ymd_and_hms(2026, 6, 25, 12, 0, 0).unwrap(),
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

fn service(db: PgPool) -> CommentService {
    CommentService::new(db, fixed_clock())
}

/// Seed an org (FK target for `comment.org_id` and `events_log.org_id`).
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

/// Seed a citizen in `org` (FK target for `comment.author_id` and `comment_vote.citizen_id`).
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

/// Read the event types the service wrote to the outbox for `org`, ordered by the
/// time-ordered event id so the sequence is deterministic even under a fixed clock.
async fn outbox_event_types(db: &PgPool, org: OrgId) -> Vec<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT event_type FROM events_log WHERE org_id = $1 ORDER BY id",
    )
    .bind(org.as_uuid())
    .fetch_all(db)
    .await
    .expect("read outbox events")
}

/// Read a single comment's stored status token.
async fn status_of(db: &PgPool, comment: CommentId) -> String {
    sqlx::query_scalar::<_, String>("SELECT status FROM comment WHERE id = $1")
        .bind(comment.as_uuid())
        .fetch_one(db)
        .await
        .expect("read comment status")
}

/// Count the votes stored for a comment.
async fn vote_count(db: &PgPool, comment: CommentId) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM comment_vote WHERE comment_id = $1")
        .bind(comment.as_uuid())
        .fetch_one(db)
        .await
        .expect("count votes")
}

#[tokio::test]
async fn root_comment_persists_and_emits_comment_created() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;
    let proposal = ProposalId::new();

    let comment = svc
        .create_comment(
            org,
            proposal,
            None,
            author,
            "creche em tempo integral, por favor",
        )
        .await
        .expect("create root comment");

    assert_eq!(comment.depth, 0, "a root comment is depth 0");
    assert!(comment.parent_id.is_none());
    assert_eq!(comment.status, CommentStatus::Visible);

    // The event was written to the outbox inside the same transaction as the insert.
    assert_eq!(
        outbox_event_types(&db, org).await,
        vec!["comments.created".to_string()]
    );
}

#[tokio::test]
async fn reply_links_to_parent_with_incremented_depth() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;
    let proposal = ProposalId::new();

    let root = svc
        .create_comment(org, proposal, None, author, "proposta raiz")
        .await
        .expect("root");
    let reply = svc
        .create_comment(
            org,
            proposal,
            Some(CommentId::from_uuid(root.id)),
            author,
            "concordo plenamente",
        )
        .await
        .expect("reply");

    assert_eq!(reply.parent_id, Some(root.id), "reply links to its parent");
    assert_eq!(
        reply.depth, 1,
        "a reply is one level deeper than its parent"
    );

    let grandchild = svc
        .create_comment(
            org,
            proposal,
            Some(CommentId::from_uuid(reply.id)),
            author,
            "complementando",
        )
        .await
        .expect("grandchild");
    assert_eq!(grandchild.depth, 2);

    // Each create emitted exactly one comments.created.
    assert_eq!(
        outbox_event_types(&db, org).await,
        vec![
            "comments.created".to_string(),
            "comments.created".to_string(),
            "comments.created".to_string(),
        ]
    );
}

#[tokio::test]
async fn reply_to_missing_parent_is_validation() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;
    let proposal = ProposalId::new();

    let err = svc
        .create_comment(
            org,
            proposal,
            Some(CommentId::new()), // never inserted
            author,
            "resposta a um pai inexistente",
        )
        .await
        .expect_err("reply to a missing parent must be rejected");
    assert_eq!(err.code(), "invalid_input");

    // No comment and no event were written.
    assert!(
        outbox_event_types(&db, org).await.is_empty(),
        "a rejected reply emits nothing"
    );
}

#[tokio::test]
async fn reply_to_parent_on_another_proposal_is_validation() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;
    let proposal_a = ProposalId::new();
    let proposal_b = ProposalId::new();

    let root = svc
        .create_comment(org, proposal_a, None, author, "thread A")
        .await
        .expect("root on A");

    let err = svc
        .create_comment(
            org,
            proposal_b, // mismatched proposal
            Some(CommentId::from_uuid(root.id)),
            author,
            "tentando pular de thread",
        )
        .await
        .expect_err("a reply may not jump to a different proposal");
    assert_eq!(err.code(), "invalid_input");
}

#[tokio::test]
async fn empty_body_is_rejected_as_validation() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;

    let err = svc
        .create_comment(org, ProposalId::new(), None, author, "   ")
        .await
        .expect_err("blank body must be rejected");
    assert_eq!(err.code(), "invalid_input");
    assert!(outbox_event_types(&db, org).await.is_empty());
}

#[tokio::test]
async fn thread_depth_guard_rejects_a_reply_past_the_maximum() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;
    let proposal = ProposalId::new();

    // Build a chain down to the deepest permitted node (depth == MAX_THREAD_DEPTH).
    let mut parent = svc
        .create_comment(org, proposal, None, author, "n0")
        .await
        .expect("root");
    for level in 1..=MAX_THREAD_DEPTH {
        parent = svc
            .create_comment(
                org,
                proposal,
                Some(CommentId::from_uuid(parent.id)),
                author,
                &format!("n{level}"),
            )
            .await
            .expect("reply within depth");
        assert_eq!(parent.depth, level);
    }
    assert_eq!(parent.depth, MAX_THREAD_DEPTH);

    // One level deeper must be rejected as a domain-rule conflict.
    let err = svc
        .create_comment(
            org,
            proposal,
            Some(CommentId::from_uuid(parent.id)),
            author,
            "reply too deep",
        )
        .await
        .expect_err("a reply past the maximum depth must conflict");
    assert_eq!(err.code(), "conflict");
}

#[tokio::test]
async fn vote_is_idempotent_per_citizen_and_returns_the_real_row() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;
    let voter = seed_citizen(&db, org).await;
    let proposal = ProposalId::new();

    let comment = svc
        .create_comment(org, proposal, None, author, "vote nisto")
        .await
        .expect("comment");
    let comment_id = CommentId::from_uuid(comment.id);

    let first = svc
        .vote(comment_id, voter, VoteWeight::Up)
        .await
        .expect("first vote");
    assert_eq!(first.weight, 1);

    // Re-voting (changing the weight) must NOT create a second row, and must return the
    // SAME stored id and original creation time — never a phantom pre-generated id.
    let second = svc
        .vote(comment_id, voter, VoteWeight::Down)
        .await
        .expect("re-vote");
    assert_eq!(second.id, first.id, "the stored vote id is stable");
    assert_eq!(
        second.created_at, first.created_at,
        "created_at is preserved"
    );
    assert_eq!(second.weight, -1, "the weight was updated in place");

    assert_eq!(vote_count(&db, comment_id).await, 1, "exactly one vote row");

    // A different citizen gets their own row.
    let other = seed_citizen(&db, org).await;
    svc.vote(comment_id, other, VoteWeight::Up)
        .await
        .expect("other vote");
    assert_eq!(vote_count(&db, comment_id).await, 2);

    // Net score = -1 (this voter) + 1 (other) = 0.
    assert_eq!(svc.score(comment_id).await.expect("score"), 0);
}

#[tokio::test]
async fn vote_on_missing_comment_is_validation() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let voter = seed_citizen(&db, org).await;

    let err = svc
        .vote(CommentId::new(), voter, VoteWeight::Up)
        .await
        .expect_err("voting on a non-existent comment violates the FK");
    assert_eq!(err.code(), "invalid_input");
}

#[tokio::test]
async fn moderation_flagged_transitions_visible_comments_idempotently() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;
    let proposal = ProposalId::new();
    let other_proposal = ProposalId::new();

    let a = svc
        .create_comment(org, proposal, None, author, "comentário A")
        .await
        .expect("a");
    let b = svc
        .create_comment(
            org,
            proposal,
            Some(CommentId::from_uuid(a.id)),
            author,
            "resposta B",
        )
        .await
        .expect("b");
    // A comment on an unrelated proposal must be untouched by the flag.
    let untouched = svc
        .create_comment(org, other_proposal, None, author, "outro tópico")
        .await
        .expect("untouched");

    let envelope = EventEnvelope {
        id: EventId::new(),
        org,
        at: fixed_clock().now(),
        event: Event::ModerationFlagged { proposal },
    };

    let flagged = handle_event(&svc, &envelope)
        .await
        .expect("handle moderation.flagged")
        .expect("moderation.flagged yields a count");
    assert_eq!(flagged, 2, "both comments of the proposal were flagged");

    assert_eq!(status_of(&db, CommentId::from_uuid(a.id)).await, "flagged");
    assert_eq!(status_of(&db, CommentId::from_uuid(b.id)).await, "flagged");
    assert_eq!(
        status_of(&db, CommentId::from_uuid(untouched.id)).await,
        "visible",
        "an unrelated proposal's comment is not flagged"
    );

    // At-least-once delivery: a redundant flag transitions zero rows (idempotent).
    let again = handle_event(&svc, &envelope)
        .await
        .expect("redundant handle")
        .expect("count");
    assert_eq!(again, 0, "re-delivering the flag is a no-op");
}

#[tokio::test]
async fn handle_event_ignores_unrelated_events() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;

    let envelope = EventEnvelope {
        id: EventId::new(),
        org,
        at: fixed_clock().now(),
        event: Event::VoteCast {
            vote: VoteId::new(),
            proposal: ProposalId::new(),
        },
    };

    let result = handle_event(&svc, &envelope)
        .await
        .expect("handle unrelated event");
    assert!(result.is_none(), "non-moderation events are ignored");
}

#[tokio::test]
async fn list_thread_is_keyset_paginated_oldest_first() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;
    let proposal = ProposalId::new();

    // Create three roots; ids are UUIDv7 (time-ordered), so creation order == sort order.
    let mut created = Vec::new();
    for i in 0..3 {
        let c = svc
            .create_comment(org, proposal, None, author, &format!("comentário {i}"))
            .await
            .expect("create");
        created.push(c);
    }

    let page = svc
        .list_thread(org, proposal, None, 2)
        .await
        .expect("first page");
    assert_eq!(page.len(), 2);
    assert_eq!(page[0].id, created[0].id);
    assert_eq!(page[1].id, created[1].id);

    // Page past the second row via the keyset cursor.
    let cursor = dsoc_comments::queries::Cursor {
        at: page[1].created_at,
        id: page[1].id,
    };
    let next = svc
        .list_thread(org, proposal, Some(cursor), 2)
        .await
        .expect("second page");
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].id, created[2].id);
}

#[tokio::test]
async fn get_comment_round_trips_and_missing_is_not_found() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;
    let proposal = ProposalId::new();

    let created = svc
        .create_comment(org, proposal, None, author, "para buscar")
        .await
        .expect("create");
    let fetched = svc
        .get_comment(CommentId::from_uuid(created.id))
        .await
        .expect("get");
    assert_eq!(fetched.id, created.id);
    assert_eq!(fetched.body, "para buscar");

    let err = svc
        .get_comment(CommentId::new())
        .await
        .expect_err("absent comment must be NotFound");
    assert_eq!(err.code(), "not_found");
}

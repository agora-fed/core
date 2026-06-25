//! Integration tests for `dsoc-moderation` against a real PostgreSQL (TESTING.md: no
//! mocked database). Each test isolates its data behind a freshly created `org` and
//! unique ids; time is injected via a `FixedClock`; events are captured with
//! `RecordingEventBus`. Set `$DATABASE_URL` to a database with the migrations applied.
//!
//! This is test-support code: panicking on a setup failure (a poisoned fixture, an
//! absent `$DATABASE_URL`) is the correct, loud behavior, so `unwrap`/`expect` are
//! allowed here even outside `#[test]` helper functions.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use chrono::{DateTime, Utc};
use dsoc_core::clock::Clock;
use dsoc_core::error::Error;
use dsoc_core::events::{Event, EventEnvelope};
use dsoc_core::ids::{CommentId, EventId, MandateId, OrgId, ProposalId};
use dsoc_core::testing::RecordingEventBus;
use dsoc_db::Db;
use dsoc_moderation::domain::{AppealStatus, Outcome, RuleAction, RuleKind, TargetKind};
use dsoc_moderation::service::ModerationTarget;
use dsoc_moderation::{handle_event, ModerationService};
use uuid::Uuid;

/// A deterministic clock so timestamps and ordering are reproducible (never sleeps).
struct FixedClock(DateTime<Utc>);
impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

fn fixed_clock() -> Arc<dyn Clock> {
    Arc::new(FixedClock(
        DateTime::parse_from_rfc3339("2026-06-25T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc),
    ))
}

async fn pool() -> Db {
    let url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for integration tests");
    dsoc_db::connect(&url, 5)
        .await
        .expect("connect to test database")
}

/// Create an isolated `org` row and return its id (FK target for moderation rows).
async fn seed_org(db: &Db) -> OrgId {
    let org = OrgId::new();
    let slug = format!("org-{}", Uuid::now_v7());
    sqlx::query!(
        "INSERT INTO org (id, slug, name, created_at) VALUES ($1, $2, $3, now())",
        org.as_uuid(),
        slug,
        "Test Org",
    )
    .execute(db)
    .await
    .expect("seed org");
    org
}

fn harness(db: &Db) -> (Arc<RecordingEventBus>, ModerationService) {
    let bus = Arc::new(RecordingEventBus::new());
    let svc = ModerationService::new(db.clone(), fixed_clock(), bus.clone());
    (bus, svc)
}

#[tokio::test]
async fn matching_rule_flags_emits_and_audits() {
    let db = pool().await;
    let org = seed_org(&db).await;
    let (bus, svc) = harness(&db);

    svc.create_rule(org, RuleKind::Keyword, "golpe", RuleAction::Reject)
        .await
        .expect("create rule");

    let proposal = ProposalId::new();
    let decision = svc
        .evaluate(
            org,
            ModerationTarget::Proposal(proposal),
            "isto e um GOLPE contra a democracia",
        )
        .await
        .expect("evaluate");

    // Outcome flagged, with the matching rule recorded for audit.
    assert_eq!(decision.outcome, Outcome::Flagged);
    assert_eq!(decision.target_kind, TargetKind::Proposal);
    assert_eq!(decision.target_id, proposal.as_uuid());
    assert!(decision.rule_id.is_some());

    // moderation.flagged emitted for the proposal.
    let published = bus.published();
    assert_eq!(published.len(), 1);
    assert!(matches!(
        published[0].event,
        Event::ModerationFlagged { proposal: p } if p == proposal
    ));

    // The decision is durably persisted (auditable).
    let fetched = svc.get_decision(decision.id).await.expect("fetch decision");
    assert_eq!(fetched.id, decision.id);
    assert_eq!(fetched.outcome, Outcome::Flagged);
}

#[tokio::test]
async fn clean_content_clears_and_emits_cleared() {
    let db = pool().await;
    let org = seed_org(&db).await;
    let (bus, svc) = harness(&db);

    svc.create_rule(org, RuleKind::Keyword, "spam", RuleAction::Flag)
        .await
        .expect("create rule");

    let proposal = ProposalId::new();
    let decision = svc
        .evaluate(
            org,
            ModerationTarget::Proposal(proposal),
            "uma proposta civica legitima e construtiva",
        )
        .await
        .expect("evaluate");

    assert_eq!(decision.outcome, Outcome::Cleared);
    assert!(decision.rule_id.is_none());
    assert!(matches!(
        bus.published()[0].event,
        Event::ModerationCleared { proposal: p } if p == proposal
    ));
}

#[tokio::test]
async fn no_rules_still_writes_a_cleared_decision() {
    // Decisions are never silently dropped: with an empty ruleset we still audit a clear.
    let db = pool().await;
    let org = seed_org(&db).await;
    let (bus, svc) = harness(&db);

    let proposal = ProposalId::new();
    let decision = svc
        .evaluate(org, ModerationTarget::Proposal(proposal), "anything at all")
        .await
        .expect("evaluate");

    assert_eq!(decision.outcome, Outcome::Cleared);
    assert_eq!(bus.count(), 1);
    assert!(svc.get_decision(decision.id).await.is_ok());
}

#[tokio::test]
async fn caps_ratio_signal_flags_shouting() {
    let db = pool().await;
    let org = seed_org(&db).await;
    let (_bus, svc) = harness(&db);

    svc.create_rule(org, RuleKind::CapsRatio, "0.8", RuleAction::Flag)
        .await
        .expect("create caps rule");

    let proposal = ProposalId::new();
    let decision = svc
        .evaluate(
            org,
            ModerationTarget::Proposal(proposal),
            "PAREM COM ISSO JA",
        )
        .await
        .expect("evaluate");
    assert_eq!(decision.outcome, Outcome::Flagged);
}

#[tokio::test]
async fn consuming_comment_created_audits_comment_target() {
    let db = pool().await;
    let org = seed_org(&db).await;
    let (bus, svc) = harness(&db);

    svc.create_rule(org, RuleKind::Keyword, "ataque", RuleAction::Reject)
        .await
        .expect("create rule");

    let comment = CommentId::new();
    let proposal = ProposalId::new();
    let envelope = EventEnvelope {
        id: EventId::new(),
        org,
        at: fixed_clock().now(),
        event: Event::CommentCreated { comment, proposal },
    };

    let decision = handle_event(&svc, &envelope, "um ATAQUE pessoal e ofensivo")
        .await
        .expect("handle event")
        .expect("comment.created is consumed");

    // Audited as a comment, but the emitted event is keyed by the parent proposal.
    assert_eq!(decision.target_kind, TargetKind::Comment);
    assert_eq!(decision.target_id, comment.as_uuid());
    assert!(matches!(
        bus.published()[0].event,
        Event::ModerationFlagged { proposal: p } if p == proposal
    ));
}

#[tokio::test]
async fn unrelated_event_is_ignored_without_a_decision() {
    let db = pool().await;
    let org = seed_org(&db).await;
    let (bus, svc) = harness(&db);

    let envelope = EventEnvelope {
        id: EventId::new(),
        org,
        at: fixed_clock().now(),
        event: Event::MandateOfficialOnboarded {
            mandate: MandateId::new(),
        },
    };

    let outcome = handle_event(&svc, &envelope, "irrelevant")
        .await
        .expect("handle event");
    assert!(outcome.is_none());
    assert_eq!(bus.count(), 0);
}

#[tokio::test]
async fn appeal_transitions_open_to_granted() {
    let db = pool().await;
    let org = seed_org(&db).await;
    let (_bus, svc) = harness(&db);

    svc.create_rule(org, RuleKind::Keyword, "fraude", RuleAction::Reject)
        .await
        .expect("create rule");
    let decision = svc
        .evaluate(
            org,
            ModerationTarget::Proposal(ProposalId::new()),
            "acusacao de FRAUDE",
        )
        .await
        .expect("evaluate");

    let appeal = svc
        .file_appeal(decision.id, "discordo da decisao, peco revisao")
        .await
        .expect("file appeal");
    assert_eq!(appeal.status, AppealStatus::Open);

    let resolved = svc
        .resolve_appeal(appeal.id, AppealStatus::Granted)
        .await
        .expect("resolve appeal");
    assert_eq!(resolved.status, AppealStatus::Granted);
}

#[tokio::test]
async fn resolved_appeal_cannot_be_redecided() {
    let db = pool().await;
    let org = seed_org(&db).await;
    let (_bus, svc) = harness(&db);

    svc.create_rule(org, RuleKind::Keyword, "abuso", RuleAction::Flag)
        .await
        .expect("create rule");
    let decision = svc
        .evaluate(
            org,
            ModerationTarget::Proposal(ProposalId::new()),
            "relato de abuso",
        )
        .await
        .expect("evaluate");
    let appeal = svc
        .file_appeal(decision.id, "peco reconsideracao")
        .await
        .expect("file appeal");

    svc.resolve_appeal(appeal.id, AppealStatus::Denied)
        .await
        .expect("first resolution");

    // Re-deciding a terminal appeal is a conflict (the state machine forbids it).
    let err = svc
        .resolve_appeal(appeal.id, AppealStatus::Granted)
        .await
        .expect_err("must conflict");
    assert!(matches!(err, Error::Conflict(_)));
    assert_eq!(err.code(), "conflict");
}

#[tokio::test]
async fn appeal_on_unknown_decision_is_not_found() {
    let db = pool().await;
    let _org = seed_org(&db).await;
    let (_bus, svc) = harness(&db);

    let err = svc
        .file_appeal(Uuid::now_v7(), "razao valida")
        .await
        .expect_err("must be not found");
    assert!(matches!(err, Error::NotFound(_)));
}

#[tokio::test]
async fn empty_reason_is_rejected_as_validation() {
    let db = pool().await;
    let org = seed_org(&db).await;
    let (_bus, svc) = harness(&db);

    let decision = svc
        .evaluate(org, ModerationTarget::Proposal(ProposalId::new()), "clean")
        .await
        .expect("evaluate");
    let err = svc
        .file_appeal(decision.id, "   ")
        .await
        .expect_err("must be validation");
    assert!(matches!(err, Error::Validation(_)));
}

#[tokio::test]
async fn invalid_caps_ratio_pattern_is_rejected() {
    let db = pool().await;
    let org = seed_org(&db).await;
    let (_bus, svc) = harness(&db);

    let err = svc
        .create_rule(org, RuleKind::CapsRatio, "not-a-number", RuleAction::Flag)
        .await
        .expect_err("must be validation");
    assert!(matches!(err, Error::Validation(_)));
}

#[tokio::test]
async fn decisions_list_is_scoped_and_keyset_paginated() {
    let db = pool().await;
    let org = seed_org(&db).await;
    let other = seed_org(&db).await;
    let (_bus, svc) = harness(&db);

    // Three decisions in our org, one in another org.
    for _ in 0..3 {
        svc.evaluate(org, ModerationTarget::Proposal(ProposalId::new()), "ok")
            .await
            .expect("evaluate");
    }
    svc.evaluate(other, ModerationTarget::Proposal(ProposalId::new()), "ok")
        .await
        .expect("evaluate other");

    let page = svc.list_decisions(org, None, 2).await.expect("list");
    assert_eq!(page.len(), 2);
    // Newest-first ordering.
    assert!(page[0].created_at >= page[1].created_at);

    // Every returned row belongs to our org (tenant isolation).
    assert!(page.iter().all(|d| d.org_id == org.as_uuid()));

    let cursor = dsoc_moderation::queries::Cursor {
        at: page[1].created_at,
        id: page[1].id,
    };
    let next = svc
        .list_decisions(org, Some(cursor), 10)
        .await
        .expect("list page 2");
    // Remaining decision(s) for this org only.
    assert_eq!(next.len(), 1);
    assert!(next.iter().all(|d| d.org_id == org.as_uuid()));
}

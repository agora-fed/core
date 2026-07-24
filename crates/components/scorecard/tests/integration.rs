//! Integration tests for `scorecard` against a real PostgreSQL (TESTING.md: no mocked database).
//!
//! Each test isolates its data with unique `org`/`mandate` fixtures so runs never collide, uses a
//! deterministic [`FixedClock`] (never sleeps), and a [`StubAuthz`] for the promise surface. The
//! scorecard is a PURE PROJECTION: every assertion checks that the public counters equal a fresh
//! recount of the event-sourced ledger, that the median is derived correctly, and that replays
//! (at-least-once delivery — ADR-0006) never double-count. Emissions are asserted against the
//! `events_log` rows the service wrote INSIDE the same transaction as the ledger append, proving
//! atomicity rather than trusting a post-commit publish.

// Test code: a failed `expect`/`panic`/`unwrap` is a test failure, which is the desired behaviour.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use dsoc_core::clock::Clock;
use dsoc_core::events::{Event, EventEnvelope};
use dsoc_core::ids::{CitizenId, EventId, MandateId, OrgId, ProposalId, SlaId, VoteId};
use dsoc_core::{Authorization, Error, Result, VerificationLevel};
use dsoc_db::Db;
use dsoc_scorecard::domain::{self, Outcome};
use dsoc_scorecard::queries;
use dsoc_scorecard::service::ScorecardService;
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

/// Authorization stub resolving every citizen to a fixed verification level.
#[derive(Debug)]
struct StubAuthz {
    level: VerificationLevel,
}

#[async_trait::async_trait]
impl Authorization for StubAuthz {
    async fn level(&self, _org: OrgId, _citizen: CitizenId) -> Result<VerificationLevel> {
        Ok(self.level)
    }

    async fn require(
        &self,
        _org: OrgId,
        _citizen: CitizenId,
        required: VerificationLevel,
    ) -> Result<()> {
        if self.level >= required {
            Ok(())
        } else {
            Err(Error::Forbidden("insufficient verification".to_owned()))
        }
    }
}

async fn pool() -> Db {
    let url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for integration tests");
    dsoc_db::connect(&url, 5)
        .await
        .expect("connect to test database")
}

fn service(db: Db) -> ScorecardService {
    service_with(db, VerificationLevel::Directory)
}

fn service_with(db: Db, level: VerificationLevel) -> ScorecardService {
    ScorecardService::new(db, fixed_clock(), Arc::new(StubAuthz { level }))
}

/// Seed an `org` row (FK target for `scorecard.org_id` / `mandate.org_id` / `events_log.org_id`).
async fn seed_org(db: &Db) -> OrgId {
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

/// Seed a `mandate` row (cross-crate FK target for `scorecard.mandate_id`).
async fn seed_mandate(db: &Db, org: OrgId) -> MandateId {
    let mandate = MandateId::new();
    sqlx::query(
        "INSERT INTO mandate (id, org_id, office, display_name, public_email, created_at) \
         VALUES ($1, $2, $3, $4, $5, now())",
    )
    .bind(mandate.as_uuid())
    .bind(org.as_uuid())
    .bind("vereador")
    .bind("Vereadora de Teste")
    .bind(format!("oficial-{}@exemplo.test", mandate.as_uuid()))
    .execute(db)
    .await
    .expect("seed mandate");
    mandate
}

/// The `scorecard.updated` events the service wrote to the outbox for `org`, ordered by the
/// (time-ordered) event id so the sequence is deterministic even under a fixed clock.
async fn outbox_scorecard_events(db: &Db, org: OrgId) -> Vec<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT event_type FROM events_log WHERE org_id = $1 AND topic = 'scorecard' ORDER BY id",
    )
    .bind(org.as_uuid())
    .fetch_all(db)
    .await
    .expect("read outbox events")
}

/// Assert the invariant: the persisted counters equal a fresh recount of the ledger.
async fn assert_counters_match_ledger(db: &Db, scorecard_id: Uuid, answered: i64, ignored: i64) {
    let entries = queries::list_entries(db, scorecard_id, 10_000)
        .await
        .expect("read ledger");
    let (re_answered, re_ignored) = domain::recount(&entries);
    assert_eq!(
        re_answered, answered,
        "answered must equal a ledger recount"
    );
    assert_eq!(re_ignored, ignored, "ignored must equal a ledger recount");
}

#[tokio::test]
async fn onboarding_creates_zeroed_scorecard_and_emits() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;

    let view = svc.on_onboarded(org, mandate).await.expect("onboard");
    assert_eq!(view.answered, 0);
    assert_eq!(view.ignored, 0);
    assert_eq!(view.median_response_hours, None);
    assert_eq!(view.mandate_id, mandate.as_uuid());

    assert_eq!(
        outbox_scorecard_events(&db, org).await,
        vec!["scorecard.updated".to_string()]
    );
    assert_counters_match_ledger(&db, view.id, 0, 0).await;
}

#[tokio::test]
async fn onboarding_is_idempotent_and_preserves_counters() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;

    let first = svc.on_onboarded(org, mandate).await.expect("onboard 1");
    // Record an ignored outcome so the counters are non-zero before the re-onboard.
    svc.on_sla_expired(org, mandate, SlaId::new(), fixed_clock().now())
        .await
        .expect("sla expired");

    let second = svc.on_onboarded(org, mandate).await.expect("onboard 2");
    // Same scorecard id (real stored id, never a phantom) and counters are NOT reset.
    assert_eq!(second.id, first.id);
    assert_eq!(second.ignored, 1, "re-onboarding must not reset counters");
    assert_counters_match_ledger(&db, second.id, 0, 1).await;
}

#[tokio::test]
async fn sla_expired_increments_ignored_appends_entry_and_emits() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    svc.on_onboarded(org, mandate).await.expect("onboard");

    let sla = SlaId::new();
    let occurred = Utc.with_ymd_and_hms(2026, 1, 2, 8, 0, 0).unwrap();
    let view = svc
        .on_sla_expired(org, mandate, sla, occurred)
        .await
        .expect("sla expired");

    assert_eq!(view.ignored, 1);
    assert_eq!(view.answered, 0);
    assert_eq!(view.median_response_hours, None);

    let entries = queries::list_entries(&db, view.id, 100).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].outcome, Outcome::Ignored);
    assert_eq!(entries[0].response_hours, None);
    assert_eq!(entries[0].sla_id, sla.as_uuid());
    assert_eq!(entries[0].occurred_at, occurred);

    // onboarding emit + sla-expired emit = two scorecard.updated signals.
    assert_eq!(
        outbox_scorecard_events(&db, org).await,
        vec![
            "scorecard.updated".to_string(),
            "scorecard.updated".to_string()
        ]
    );
    assert_counters_match_ledger(&db, view.id, 0, 1).await;
}

#[tokio::test]
async fn responded_increments_answered_records_latency_and_emits() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    svc.on_onboarded(org, mandate).await.expect("onboard");

    let sla = SlaId::new();
    let occurred = Utc.with_ymd_and_hms(2026, 1, 3, 10, 0, 0).unwrap();
    let view = svc
        .on_responded(org, mandate, sla, 12.5, occurred)
        .await
        .expect("responded");

    assert_eq!(view.answered, 1);
    assert_eq!(view.ignored, 0);
    assert_eq!(view.median_response_hours, Some(12.5));

    let entries = queries::list_entries(&db, view.id, 100).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].outcome, Outcome::Answered);
    assert_eq!(entries[0].response_hours, Some(12.5));
    assert_counters_match_ledger(&db, view.id, 1, 0).await;
}

#[tokio::test]
async fn median_is_correct_across_mixed_outcomes() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    svc.on_onboarded(org, mandate).await.expect("onboard");

    // Answered latencies 2, 6, 10 -> median 6; plus two ignored.
    let t = Utc.with_ymd_and_hms(2026, 1, 4, 0, 0, 0).unwrap();
    svc.on_responded(org, mandate, SlaId::new(), 2.0, t)
        .await
        .unwrap();
    svc.on_responded(org, mandate, SlaId::new(), 10.0, t)
        .await
        .unwrap();
    svc.on_responded(org, mandate, SlaId::new(), 6.0, t)
        .await
        .unwrap();
    svc.on_sla_expired(org, mandate, SlaId::new(), t)
        .await
        .unwrap();
    let view = svc
        .on_sla_expired(org, mandate, SlaId::new(), t)
        .await
        .unwrap();

    assert_eq!(view.answered, 3);
    assert_eq!(view.ignored, 2);
    assert_eq!(view.median_response_hours, Some(6.0));
    assert_counters_match_ledger(&db, view.id, 3, 2).await;

    // Reading back via the public projection yields the same derived median.
    let read = svc.get_by_mandate(mandate).await.expect("read");
    assert_eq!(read.median_response_hours, Some(6.0));
}

#[tokio::test]
async fn median_is_none_with_only_ignored_outcomes() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    svc.on_onboarded(org, mandate).await.expect("onboard");

    let t = fixed_clock().now();
    svc.on_sla_expired(org, mandate, SlaId::new(), t)
        .await
        .unwrap();
    let view = svc
        .on_sla_expired(org, mandate, SlaId::new(), t)
        .await
        .unwrap();
    assert_eq!(view.ignored, 2);
    assert_eq!(view.median_response_hours, None);
}

#[tokio::test]
async fn replaying_same_sla_id_is_idempotent_and_does_not_double_count() {
    // Accountability regression guard (ADR-0006): the ledger dedupes by sla_id, so an at-least-once
    // redelivery of the SAME outcome appends nothing and emits no second signal. Exactly one entry
    // and one counter increment survive.
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    svc.on_onboarded(org, mandate).await.expect("onboard");

    let sla = SlaId::new();
    let t = fixed_clock().now();
    let first = svc
        .on_sla_expired(org, mandate, sla, t)
        .await
        .expect("first");
    let again = svc
        .on_sla_expired(org, mandate, sla, t)
        .await
        .expect("replay");

    assert_eq!(first.ignored, 1);
    assert_eq!(again.ignored, 1, "replay must not double-count");

    let entries = queries::list_entries(&db, first.id, 100).await.unwrap();
    assert_eq!(
        entries.len(),
        1,
        "replay must not append a second ledger row"
    );

    // onboarding emit + first sla-expired emit = two; the replay emits nothing.
    assert_eq!(
        outbox_scorecard_events(&db, org).await,
        vec![
            "scorecard.updated".to_string(),
            "scorecard.updated".to_string()
        ]
    );
    assert_counters_match_ledger(&db, first.id, 0, 1).await;
}

#[tokio::test]
async fn responded_replay_with_different_latency_keeps_first_value() {
    // The sla_id dedupe means the first recorded latency is authoritative; a redelivery cannot
    // silently overwrite it (no double-count, no value drift).
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    svc.on_onboarded(org, mandate).await.expect("onboard");

    let sla = SlaId::new();
    let t = fixed_clock().now();
    svc.on_responded(org, mandate, sla, 4.0, t)
        .await
        .expect("first");
    let again = svc
        .on_responded(org, mandate, sla, 99.0, t)
        .await
        .expect("replay");

    assert_eq!(again.answered, 1);
    assert_eq!(again.median_response_hours, Some(4.0), "first latency wins");
}

#[tokio::test]
async fn outcome_before_onboarding_creates_scorecard_resiliently() {
    // Out-of-order, at-least-once delivery: an SLA outcome may arrive before the onboarding event.
    // The projection must still create the scorecard and record the outcome.
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;

    let view = svc
        .on_sla_expired(org, mandate, SlaId::new(), fixed_clock().now())
        .await
        .expect("sla expired before onboarding");
    assert_eq!(view.ignored, 1);

    // A later onboarding finds the existing scorecard and keeps the counters.
    let onboarded = svc.on_onboarded(org, mandate).await.expect("late onboard");
    assert_eq!(onboarded.id, view.id);
    assert_eq!(onboarded.ignored, 1);
}

#[tokio::test]
async fn rejects_negative_or_nonfinite_latency() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    svc.on_onboarded(org, mandate).await.expect("onboard");

    let err = svc
        .on_responded(org, mandate, SlaId::new(), -1.0, fixed_clock().now())
        .await
        .expect_err("negative latency must be rejected");
    assert_eq!(err.code(), "invalid_input");
}

#[tokio::test]
async fn get_by_mandate_is_not_found_when_absent() {
    let db = pool().await;
    let svc = service(db.clone());
    let err = svc
        .get_by_mandate(MandateId::new())
        .await
        .expect_err("absent scorecard must be NotFound");
    assert_eq!(err.code(), "not_found");
}

#[tokio::test]
async fn list_scorecards_is_keyset_paginated() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let m1 = seed_mandate(&db, org).await;
    let m2 = seed_mandate(&db, org).await;
    svc.on_onboarded(org, m1).await.unwrap();
    svc.on_onboarded(org, m2).await.unwrap();

    let all = svc.list(org, None, 50).await.expect("list");
    assert_eq!(all.len(), 2);

    // First page of one, then a cursor past it.
    let first_page = svc.list(org, None, 1).await.expect("page 1");
    assert_eq!(first_page.len(), 1);
    let cursor = queries::Cursor {
        at: first_page[0].created_at,
        id: first_page[0].id,
    };
    let second_page = svc.list(org, Some(cursor), 50).await.expect("page 2");
    assert_eq!(second_page.len(), 1);
    assert_ne!(second_page[0].id, first_page[0].id);
}

// --- event dispatch (handle_event) --------------------------------------------------

fn envelope(org: OrgId, at: DateTime<Utc>, event: Event) -> EventEnvelope {
    EventEnvelope {
        id: EventId::new(),
        org,
        at,
        event,
    }
}

#[tokio::test]
async fn handle_event_dispatches_each_consumed_event() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    let at = fixed_clock().now();

    // Onboarded.
    let onboarded = dsoc_scorecard::handle_event(
        &svc,
        &envelope(org, at, Event::MandateOfficialOnboarded { mandate }),
        None,
    )
    .await
    .expect("onboarded")
    .expect("yields a view");
    assert_eq!(onboarded.answered, 0);

    // SLA expired.
    let expired = dsoc_scorecard::handle_event(
        &svc,
        &envelope(
            org,
            at,
            Event::ConsequenceSlaExpired {
                sla: SlaId::new(),
                mandate,
            },
        ),
        None,
    )
    .await
    .expect("expired")
    .expect("yields a view");
    assert_eq!(expired.ignored, 1);

    // Responded with latency delivered alongside the slim event.
    let responded = dsoc_scorecard::handle_event(
        &svc,
        &envelope(
            org,
            at,
            Event::ConsequenceOfficialResponded {
                sla: SlaId::new(),
                mandate,
            },
        ),
        Some(8.0),
    )
    .await
    .expect("responded")
    .expect("yields a view");
    assert_eq!(responded.answered, 1);
    assert_eq!(responded.median_response_hours, Some(8.0));
}

#[tokio::test]
async fn handle_event_ignores_unrelated_events() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let at = fixed_clock().now();

    let out = dsoc_scorecard::handle_event(
        &svc,
        &envelope(
            org,
            at,
            Event::VoteCast {
                vote: VoteId::new(),
                proposal: ProposalId::new(),
            },
        ),
        None,
    )
    .await
    .expect("ignored event handled");
    assert!(
        out.is_none(),
        "unrelated events produce no projection change"
    );
    assert!(outbox_scorecard_events(&db, org).await.is_empty());
}

#[tokio::test]
async fn handle_event_responded_without_latency_is_validation_error() {
    let db = pool().await;
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;

    let err = dsoc_scorecard::handle_event(
        &svc,
        &envelope(
            org,
            fixed_clock().now(),
            Event::ConsequenceOfficialResponded {
                sla: SlaId::new(),
                mandate,
            },
        ),
        None,
    )
    .await
    .expect_err("responded requires a latency");
    assert_eq!(err.code(), "invalid_input");
}

// --- promise surface (authorized mutations) -----------------------------------------

#[tokio::test]
async fn record_promise_requires_directory_verification() {
    let db = pool().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    // Create the scorecard first (promises attach to it).
    service(db.clone())
        .on_onboarded(org, mandate)
        .await
        .expect("onboard");

    // An email-only caller is forbidden.
    let weak = service_with(db.clone(), VerificationLevel::Email);
    let err = weak
        .record_promise(CitizenId::new(), mandate, "construir a creche")
        .await
        .expect_err("under-verified caller must be forbidden");
    assert_eq!(err.code(), "forbidden");

    // A directory-verified official succeeds.
    let strong = service_with(db.clone(), VerificationLevel::Directory);
    let promise = strong
        .record_promise(CitizenId::new(), mandate, "  construir a creche  ")
        .await
        .expect("record promise");
    assert_eq!(promise.text, "construir a creche", "text is trimmed");
    assert!(!promise.delivered);
}

#[tokio::test]
async fn record_promise_on_unknown_mandate_is_not_found() {
    let db = pool().await;
    let svc = service(db.clone());
    let err = svc
        .record_promise(CitizenId::new(), MandateId::new(), "x")
        .await
        .expect_err("no scorecard -> NotFound");
    assert_eq!(err.code(), "not_found");
}

#[tokio::test]
async fn record_promise_creates_scorecard_when_absent() {
    // Uma promessa pode ser o PRIMEIRO artefato público de um mandato — antes de
    // qualquer evento da consequência ter projetado um scorecard. Registrar deve
    // criar o scorecard sob demanda (não 404).
    let db = pool().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    // Repare: SEM on_onboarded — o scorecard ainda não existe.
    let strong = service_with(db.clone(), VerificationLevel::Directory);
    let promise = strong
        .record_promise(CitizenId::new(), mandate, "primeira promessa")
        .await
        .expect("registrar deve criar o scorecard");
    assert_eq!(promise.text, "primeira promessa");
    assert!(!promise.delivered);
    // O scorecard passou a existir e a promessa aparece na listagem pública.
    let promises = strong
        .list_promises(mandate, None, 10)
        .await
        .expect("list promises");
    assert_eq!(promises.len(), 1);
}

#[tokio::test]
async fn record_promise_rejects_empty_text() {
    let db = pool().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    let svc = service(db.clone());
    svc.on_onboarded(org, mandate).await.expect("onboard");

    let err = svc
        .record_promise(CitizenId::new(), mandate, "   ")
        .await
        .expect_err("blank promise text must be rejected");
    assert_eq!(err.code(), "invalid_input");
}

#[tokio::test]
async fn deliver_promise_is_guarded_and_idempotent() {
    let db = pool().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org).await;
    let svc = service(db.clone());
    svc.on_onboarded(org, mandate).await.expect("onboard");
    let promise = svc
        .record_promise(CitizenId::new(), mandate, "asfaltar a rua")
        .await
        .expect("record");
    assert!(promise.delivered_at.is_none());

    let delivered = svc
        .mark_promise_delivered(CitizenId::new(), promise.id)
        .await
        .expect("deliver");
    assert!(delivered.delivered);
    assert!(delivered.delivered_at.is_some());

    // Marking again is idempotent (0 rows affected is the already-in-target case): same state, no error.
    let again = svc
        .mark_promise_delivered(CitizenId::new(), promise.id)
        .await
        .expect("idempotent deliver");
    assert!(again.delivered);
    assert_eq!(again.delivered_at, delivered.delivered_at);

    // The promise appears in the public list.
    let promises = svc
        .list_promises(mandate, None, 50)
        .await
        .expect("list promises");
    assert_eq!(promises.len(), 1);
    assert!(promises[0].delivered);
}

#[tokio::test]
async fn deliver_unknown_promise_is_not_found() {
    let db = pool().await;
    let svc = service(db.clone());
    let err = svc
        .mark_promise_delivered(CitizenId::new(), Uuid::now_v7())
        .await
        .expect_err("absent promise must be NotFound");
    assert_eq!(err.code(), "not_found");
}

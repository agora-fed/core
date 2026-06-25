//! Integration tests for `dsoc-mandates` against a real PostgreSQL (docs/TESTING.md: no mocked
//! DB). Each test isolates its data behind a freshly generated `OrgId`/`MandateId`, injects a
//! deterministic `FixedClock` (never sleeps), and asserts cross-crate emissions by reading the
//! transactional outbox (`events_log`) — the events are written via the outbox, not the
//! fire-and-forget bus (ADR-0006), so a `RecordingEventBus` stays empty by design.
//!
//! Requires `DATABASE_URL` and the baseline + `0200_mandates_lifecycle.sql` applied (the harness
//! does both).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use dsoc_core::events::Event;
use dsoc_core::ids::{CitizenId, MandateId, OrgId, SpaceId};
use dsoc_core::testing::RecordingEventBus;
use dsoc_core::traits::Space;
use dsoc_core::{Authorization, Clock, Error, EventBus, Result, VerificationLevel};
use dsoc_db::Db;
use dsoc_mandates::domain::{OnboardingStatus, INVITATION_TTL_HOURS};
use dsoc_mandates::{MandateRegistry, MandatesSpace, OfficeDraft};

/// Deterministic clock — onboarding/expiry timing is reproducible without sleeping.
#[derive(Debug, Clone, Copy)]
struct FixedClock(DateTime<Utc>);
impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
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

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-06-25T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

async fn connect() -> Db {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    dsoc_db::connect(&url, 5)
        .await
        .expect("connect to Postgres")
}

/// Build a registry at a given clock instant and caller verification level.
fn registry(
    db: Db,
    at: DateTime<Utc>,
    level: VerificationLevel,
) -> (MandateRegistry, Arc<RecordingEventBus>) {
    let clock: Arc<dyn Clock> = Arc::new(FixedClock(at));
    let bus = Arc::new(RecordingEventBus::new());
    let authz = Arc::new(StubAuthz { level });
    let svc = MandateRegistry::new(db, clock, bus.clone() as Arc<dyn EventBus>, authz);
    (svc, bus)
}

/// Insert a fresh isolated org and return its id.
async fn seed_org(db: &Db) -> OrgId {
    let org = OrgId::new();
    sqlx::query("INSERT INTO org (id, slug, name, created_at) VALUES ($1, $2, $3, $4)")
        .bind(org.as_uuid())
        .bind(format!("org-{}", org.as_uuid().simple()))
        .bind("Prefeitura Teste")
        .bind(now())
        .execute(db)
        .await
        .expect("seed org");
    org
}

/// Insert a fresh mandate (not yet onboarded) and return its id.
async fn seed_mandate(db: &Db, org: OrgId, email: &str) -> MandateId {
    let mandate = MandateId::new();
    sqlx::query(
        "INSERT INTO mandate \
         (id, org_id, office, display_name, public_email, is_candidate, onboarded_at, created_at) \
         VALUES ($1, $2, $3, $4, $5, false, NULL, $6)",
    )
    .bind(mandate.as_uuid())
    .bind(org.as_uuid())
    .bind("vereador")
    .bind("Vereadora Fulana")
    .bind(email)
    .bind(now())
    .execute(db)
    .await
    .expect("seed mandate");
    mandate
}

async fn count_events(db: &Db, org: OrgId, event_type: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM events_log WHERE org_id = $1 AND event_type = $2")
        .bind(org.as_uuid())
        .bind(event_type)
        .fetch_one(db)
        .await
        .unwrap()
}

// ---------------------------------------------------------------------------
// invite
// ---------------------------------------------------------------------------

#[tokio::test]
async fn invite_hashes_token_and_emits_invited() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org, "vereadora@camara.test").await;
    let (svc, bus) = registry(db.clone(), now(), VerificationLevel::Directory);

    let inv = svc
        .invite(org, CitizenId::new(), mandate)
        .await
        .expect("invite");

    // The returned token is the one-time plaintext; the column stores only its SHA-256 hash.
    let stored_hash: String =
        sqlx::query_scalar("SELECT token_hash FROM mandate_invitation WHERE id = $1")
            .bind(inv.id)
            .fetch_one(&db)
            .await
            .unwrap();
    let expected_hash: String =
        sqlx::query_scalar("SELECT encode(digest($1::text, 'sha256'), 'hex')")
            .bind(&inv.token)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(
        stored_hash, expected_hash,
        "stored hash must be sha256(token)"
    );
    assert_ne!(
        stored_hash, inv.token,
        "plaintext token must never be stored"
    );
    assert_eq!(inv.public_email, "vereadora@camara.test");
    assert_eq!(inv.sent_at, now());

    // Emitted through the transactional outbox, not the fire-and-forget bus.
    assert_eq!(bus.count(), 0);
    assert_eq!(count_events(&db, org, "mandates.official.invited").await, 1);

    let payload: String = sqlx::query_scalar(
        "SELECT payload::text FROM events_log WHERE org_id = $1 AND event_type = 'mandates.official.invited'",
    )
    .bind(org.as_uuid())
    .fetch_one(&db)
    .await
    .unwrap();
    match serde_json::from_str::<Event>(&payload).unwrap() {
        Event::MandateOfficialInvited { mandate: m } => assert_eq!(m, mandate),
        other => panic!("unexpected event: {other:?}"),
    }
}

#[tokio::test]
async fn invite_requires_directory_level_caller() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org, "x@camara.test").await;
    // Caller only holds Email — below the Directory bar for mutations.
    let (svc, _bus) = registry(db.clone(), now(), VerificationLevel::Email);

    let err = svc
        .invite(org, CitizenId::new(), mandate)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Forbidden(_)));
    // No invitation persisted, no event emitted.
    let invites: i64 =
        sqlx::query_scalar("SELECT count(*) FROM mandate_invitation WHERE mandate_id = $1")
            .bind(mandate.as_uuid())
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(invites, 0);
    assert_eq!(count_events(&db, org, "mandates.official.invited").await, 0);
}

#[tokio::test]
async fn invite_unknown_mandate_is_not_found() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let (svc, _bus) = registry(db, now(), VerificationLevel::Directory);
    let err = svc
        .invite(org, CitizenId::new(), MandateId::new())
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

// ---------------------------------------------------------------------------
// accept / onboarding
// ---------------------------------------------------------------------------

#[tokio::test]
async fn accept_valid_token_onboards_and_emits_onboarded() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org, "prefeito@pref.test").await;
    let (svc, _bus) = registry(db.clone(), now(), VerificationLevel::Directory);

    let inv = svc
        .invite(org, CitizenId::new(), mandate)
        .await
        .expect("invite");
    let onboarding = svc
        .accept_invitation(org, &inv.token)
        .await
        .expect("accept");
    assert_eq!(onboarding.mandate, mandate);
    assert_eq!(onboarding.onboarded_at, now());

    // onboarded_at is set on the mandate row.
    let onboarded_at: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT onboarded_at FROM mandate WHERE id = $1")
            .bind(mandate.as_uuid())
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(onboarded_at, Some(now()));

    // The invitation is marked accepted.
    let accepted_at: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT accepted_at FROM mandate_invitation WHERE id = $1")
            .bind(inv.id)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(accepted_at, Some(now()));

    // A directory-grade identity binding was recorded with the invitation as evidence.
    let (level, evidence): (String, Option<String>) = sqlx::query_as(
        "SELECT verification_level, evidence_ref FROM mandate_identity_binding WHERE mandate_id = $1",
    )
    .bind(mandate.as_uuid())
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(level, "directory");
    assert_eq!(evidence, Some(format!("invitation:{}", inv.id)));

    // The onboarded event landed in the outbox.
    assert_eq!(
        count_events(&db, org, "mandates.official.onboarded").await,
        1
    );
    let payload: String = sqlx::query_scalar(
        "SELECT payload::text FROM events_log WHERE org_id = $1 AND event_type = 'mandates.official.onboarded'",
    )
    .bind(org.as_uuid())
    .fetch_one(&db)
    .await
    .unwrap();
    match serde_json::from_str::<Event>(&payload).unwrap() {
        Event::MandateOfficialOnboarded { mandate: m } => assert_eq!(m, mandate),
        other => panic!("unexpected event: {other:?}"),
    }
}

#[tokio::test]
async fn double_accept_same_token_is_conflict() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org, "a@camara.test").await;
    let (svc, _bus) = registry(db.clone(), now(), VerificationLevel::Directory);

    let inv = svc
        .invite(org, CitizenId::new(), mandate)
        .await
        .expect("invite");
    svc.accept_invitation(org, &inv.token)
        .await
        .expect("first accept");

    let err = svc.accept_invitation(org, &inv.token).await.unwrap_err();
    assert!(
        matches!(err, Error::Conflict(_)),
        "double-onboard must Conflict"
    );

    // Still exactly one onboarded event and one binding (no duplicate side effects).
    assert_eq!(
        count_events(&db, org, "mandates.official.onboarded").await,
        1
    );
    let bindings: i64 =
        sqlx::query_scalar("SELECT count(*) FROM mandate_identity_binding WHERE mandate_id = $1")
            .bind(mandate.as_uuid())
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(bindings, 1);
}

#[tokio::test]
async fn second_invite_after_onboarding_cannot_reonboard() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org, "b@camara.test").await;
    let (svc, _bus) = registry(db.clone(), now(), VerificationLevel::Directory);

    let first = svc
        .invite(org, CitizenId::new(), mandate)
        .await
        .expect("invite 1");
    svc.accept_invitation(org, &first.token)
        .await
        .expect("accept 1");

    // A fresh, pending invitation exists, but the mandate is already onboarded.
    let second = svc
        .invite(org, CitizenId::new(), mandate)
        .await
        .expect("invite 2");
    let err = svc.accept_invitation(org, &second.token).await.unwrap_err();
    assert!(
        matches!(err, Error::Conflict(_)),
        "already-onboarded must Conflict"
    );

    // onboarded_at unchanged; still a single onboarded event.
    assert_eq!(
        count_events(&db, org, "mandates.official.onboarded").await,
        1
    );
}

#[tokio::test]
async fn unknown_token_is_not_found_no_state_change() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org, "c@camara.test").await;
    let (svc, _bus) = registry(db.clone(), now(), VerificationLevel::Directory);

    // A well-formed but never-issued token.
    let err = svc
        .accept_invitation(
            org,
            &format!("{}{}", Uuid::now_v7().simple(), Uuid::now_v7().simple()),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));

    let onboarded_at: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT onboarded_at FROM mandate WHERE id = $1")
            .bind(mandate.as_uuid())
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(onboarded_at, None, "no state change on unknown token");
    assert_eq!(
        count_events(&db, org, "mandates.official.onboarded").await,
        0
    );
}

#[tokio::test]
async fn blank_token_is_validation_error() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let (svc, _bus) = registry(db, now(), VerificationLevel::Directory);
    let err = svc.accept_invitation(org, "   ").await.unwrap_err();
    assert!(matches!(err, Error::Validation(_)));
}

#[tokio::test]
async fn expired_token_is_validation_no_state_change() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org, "d@camara.test").await;

    // Invite at T0.
    let (inviter, _bus) = registry(db.clone(), now(), VerificationLevel::Directory);
    let inv = inviter
        .invite(org, CitizenId::new(), mandate)
        .await
        .expect("invite");

    // Accept one second past the TTL window -> expired.
    let later = now() + Duration::hours(INVITATION_TTL_HOURS) + Duration::seconds(1);
    let (acceptor, _bus2) = registry(db.clone(), later, VerificationLevel::Directory);
    let err = acceptor
        .accept_invitation(org, &inv.token)
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::Validation(_)),
        "expired token must be Validation"
    );

    // No onboarding occurred.
    let onboarded_at: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT onboarded_at FROM mandate WHERE id = $1")
            .bind(mandate.as_uuid())
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(onboarded_at, None);
    assert_eq!(
        count_events(&db, org, "mandates.official.onboarded").await,
        0
    );
}

#[tokio::test]
async fn token_exactly_at_ttl_boundary_still_onboards() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org, "e@camara.test").await;

    let (inviter, _bus) = registry(db.clone(), now(), VerificationLevel::Directory);
    let inv = inviter
        .invite(org, CitizenId::new(), mandate)
        .await
        .expect("invite");

    // Exactly at sent_at + TTL: still valid (inclusive boundary).
    let boundary = now() + Duration::hours(INVITATION_TTL_HOURS);
    let (acceptor, _bus2) = registry(db.clone(), boundary, VerificationLevel::Directory);
    let onboarding = acceptor
        .accept_invitation(org, &inv.token)
        .await
        .expect("accept at boundary");
    assert_eq!(onboarding.onboarded_at, boundary);
}

// ---------------------------------------------------------------------------
// identity bindings
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bind_identity_records_and_emits_verified() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org, "f@camara.test").await;
    let (svc, _bus) = registry(db.clone(), now(), VerificationLevel::Directory);

    let binding = svc
        .bind_identity(
            org,
            CitizenId::new(),
            mandate,
            VerificationLevel::Strong,
            Some("tse:registro-123"),
        )
        .await
        .expect("bind identity");
    assert_eq!(binding.level, VerificationLevel::Strong);
    assert_eq!(binding.evidence_ref.as_deref(), Some("tse:registro-123"));
    assert_eq!(binding.verified_at, now());

    assert_eq!(
        count_events(&db, org, "mandates.identity.verified").await,
        1
    );

    // It appears in the keyset history.
    let history = svc
        .list_identity_bindings(mandate, None, Some(10))
        .await
        .expect("history");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].level, VerificationLevel::Strong);
}

#[tokio::test]
async fn bind_identity_requires_authorized_caller() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org, "g@camara.test").await;
    let (svc, _bus) = registry(db.clone(), now(), VerificationLevel::Email);

    let err = svc
        .bind_identity(
            org,
            CitizenId::new(),
            mandate,
            VerificationLevel::Strong,
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Forbidden(_)));
    assert_eq!(
        count_events(&db, org, "mandates.identity.verified").await,
        0
    );
}

#[tokio::test]
async fn bind_identity_unknown_mandate_is_not_found() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let (svc, _bus) = registry(db, now(), VerificationLevel::Directory);
    let err = svc
        .bind_identity(
            org,
            CitizenId::new(),
            MandateId::new(),
            VerificationLevel::Strong,
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

// ---------------------------------------------------------------------------
// offices
// ---------------------------------------------------------------------------

#[tokio::test]
async fn add_and_list_offices() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org, "h@camara.test").await;
    let (svc, _bus) = registry(db, now(), VerificationLevel::Directory);

    let start = chrono::NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
    let end = chrono::NaiveDate::from_ymd_opt(2028, 12, 31).unwrap();
    let office = svc
        .add_office(
            org,
            CitizenId::new(),
            mandate,
            OfficeDraft {
                office: "vereador".to_owned(),
                district: Some("Centro".to_owned()),
                term_start: Some(start),
                term_end: Some(end),
            },
        )
        .await
        .expect("add office");
    assert_eq!(office.office, "vereador");
    assert_eq!(office.district.as_deref(), Some("Centro"));
    assert_eq!(office.term_start, Some(start));

    let offices = svc
        .list_offices(mandate, None, Some(10))
        .await
        .expect("list");
    assert_eq!(offices.len(), 1);
    assert_eq!(offices[0].id, office.id);
}

#[tokio::test]
async fn add_office_rejects_blank_office() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org, "i@camara.test").await;
    let (svc, _bus) = registry(db, now(), VerificationLevel::Directory);
    let err = svc
        .add_office(
            org,
            CitizenId::new(),
            mandate,
            OfficeDraft {
                office: "   ".to_owned(),
                ..OfficeDraft::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Validation(_)));
}

// ---------------------------------------------------------------------------
// get_mandate — derived onboarding status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_mandate_derives_onboarding_status_through_lifecycle() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let mandate = seed_mandate(&db, org, "j@camara.test").await;
    let (svc, _bus) = registry(db, now(), VerificationLevel::Directory);

    // No invite yet -> NotInvited; the public handle is stable and prefixed.
    let view = svc.get_mandate(org, mandate).await.expect("get");
    assert_eq!(view.status, OnboardingStatus::NotInvited);
    assert!(view.public_handle.starts_with("m-"));

    // After invite -> Invited.
    let inv = svc
        .invite(org, CitizenId::new(), mandate)
        .await
        .expect("invite");
    let view = svc.get_mandate(org, mandate).await.expect("get");
    assert_eq!(view.status, OnboardingStatus::Invited);

    // After accept -> Onboarded.
    svc.accept_invitation(org, &inv.token)
        .await
        .expect("accept");
    let view = svc.get_mandate(org, mandate).await.expect("get");
    assert_eq!(view.status, OnboardingStatus::Onboarded);
}

#[tokio::test]
async fn get_mandate_unknown_is_not_found() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let (svc, _bus) = registry(db, now(), VerificationLevel::Directory);
    let err = svc.get_mandate(org, MandateId::new()).await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

// ---------------------------------------------------------------------------
// Space trait
// ---------------------------------------------------------------------------

#[tokio::test]
async fn space_gates_components_and_kind() {
    let db = connect().await;
    let space = MandatesSpace::new(db);
    assert_eq!(space.kind(), "mandates");
    // Accountability-loop components are allowed; unrelated ones are rejected.
    assert!(space.allows_component("proposals"));
    assert!(space.allows_component("scorecard"));
    assert!(!space.allows_component("budgets"));
    assert!(!space.allows_component("meetings"));
}

#[tokio::test]
async fn space_ensure_open_checks_org_existence() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let space = MandatesSpace::new(db);

    space
        .ensure_open(org, SpaceId::new())
        .await
        .expect("existing org is open");

    let err = space
        .ensure_open(OrgId::new(), SpaceId::new())
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

// ---------------------------------------------------------------------------
// event consumption (idempotent)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn consume_handles_auth_verification_upgraded_and_ignores_others() {
    use dsoc_core::events::EventEnvelope;
    use dsoc_core::ids::{EventId, ProposalId};

    let db = connect().await;
    let (svc, _bus) = registry(db, now(), VerificationLevel::Directory);

    let handled = EventEnvelope {
        id: EventId::new(),
        org: OrgId::new(),
        at: now(),
        event: Event::AuthVerificationUpgraded {
            citizen: CitizenId::new(),
        },
    };
    assert!(svc.consume(&handled).expect("consume"));
    // Idempotent: a re-delivery is a harmless no-op that still reports handled.
    assert!(svc.consume(&handled).expect("consume again"));

    let ignored = EventEnvelope {
        id: EventId::new(),
        org: OrgId::new(),
        at: now(),
        event: Event::ModerationFlagged {
            proposal: ProposalId::new(),
        },
    };
    assert!(!svc.consume(&ignored).expect("ignore"));
}

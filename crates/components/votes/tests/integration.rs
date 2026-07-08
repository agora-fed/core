//! Integration tests for `dsoc-votes` against a real PostgreSQL (TESTING.md: no mocked DB).
//! Each test isolates its data behind a unique `OrgId` / fresh `ProposalId`, uses a deterministic
//! `FixedClock` (never sleeps), and asserts emitted events by reading the `events_log` outbox.
//!
//! Requires `DATABASE_URL` and the `0310_votes_*` migration applied (the harness does both).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};

use dsoc_app::AppState;
use dsoc_core::events::Event;
use dsoc_core::ids::{CitizenId, OrgId, ProposalId};
use dsoc_core::testing::RecordingEventBus;
use dsoc_core::{Authorization, Clock, Error, EventBus, Result, VerificationLevel};
use dsoc_db::Db;
use dsoc_votes::{CastVoteRequest, TallyDto, VoteService};

/// Deterministic clock — timing is reproducible without sleeping.
#[derive(Debug, Clone, Copy)]
struct FixedClock(DateTime<Utc>);
impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

/// A test double for [`Authorization`] that grants a fixed level to everyone (the trait, not the
/// concrete `auth` crate, is what `votes` depends on). `require` mirrors the real semantics.
#[derive(Debug, Clone, Copy)]
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
            Err(Error::Forbidden("below required level".to_owned()))
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

/// Insert a fresh isolated org and return its id.
async fn seed_org(db: &Db) -> OrgId {
    let org = OrgId::new();
    sqlx::query("INSERT INTO org (id, slug, name, created_at) VALUES ($1, $2, $3, $4)")
        .bind(org.as_uuid())
        .bind(format!("org-{}", org.as_uuid().simple()))
        .bind("Test Org")
        .bind(now())
        .execute(db)
        .await
        .expect("seed org");
    org
}

/// Insert a citizen (votes FK-references the core `citizen` identity table).
async fn seed_citizen(db: &Db, org: OrgId) -> CitizenId {
    let citizen = CitizenId::new();
    sqlx::query(
        "INSERT INTO citizen (id, org_id, oidc_subject, verification_level, created_at) \
         VALUES ($1, $2, $3, 'email', $4)",
    )
    .bind(citizen.as_uuid())
    .bind(org.as_uuid())
    .bind(format!("sub-{}", citizen.as_uuid().simple()))
    .bind(now())
    .execute(db)
    .await
    .expect("seed citizen");
    citizen
}

fn service(db: Db) -> VoteService {
    VoteService::new(db, Arc::new(FixedClock(now())))
}

fn app_state(db: Db, level: VerificationLevel) -> AppState {
    AppState {
        db,
        bus: Arc::new(RecordingEventBus::new()) as Arc<dyn EventBus>,
        authz: Arc::new(StubAuthz { level }) as Arc<dyn Authorization>,
        clock: Arc::new(FixedClock(now())) as Arc<dyn Clock>,
        storage: None,
    }
}

async fn events_for_org(db: &Db, org: OrgId, event_type: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM events_log WHERE org_id = $1 AND event_type = $2")
        .bind(org.as_uuid())
        .bind(event_type)
        .fetch_one(db)
        .await
        .unwrap()
}

#[tokio::test]
async fn cast_inserts_increments_tally_and_emits_vote_cast() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let citizen = seed_citizen(&db, org).await;
    let proposal = ProposalId::new();

    let receipt = service(db.clone())
        .cast(org, proposal, citizen)
        .await
        .expect("cast");
    assert_eq!(receipt.support_count, 1);
    assert_eq!(receipt.proposal, proposal);

    // The protected linkage row was written exactly once.
    let votes: i64 = sqlx::query_scalar("SELECT count(*) FROM votes_vote WHERE proposal_id = $1")
        .bind(proposal.as_uuid())
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(votes, 1);

    // The aggregate tally was incremented.
    let count: i64 =
        sqlx::query_scalar("SELECT support_count FROM votes_vote_tally WHERE proposal_id = $1")
            .bind(proposal.as_uuid())
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(count, 1);

    // ADR-0006: the events landed in the outbox (`events_log`) inside the cast transaction.
    let payload: String = sqlx::query_scalar(
        "SELECT payload::text FROM events_log WHERE org_id = $1 AND event_type = 'votes.cast'",
    )
    .bind(org.as_uuid())
    .fetch_one(&db)
    .await
    .unwrap();
    match serde_json::from_str::<Event>(&payload).unwrap() {
        Event::VoteCast { vote, proposal: p } => {
            assert_eq!(vote, receipt.vote);
            assert_eq!(p, proposal);
        }
        other => panic!("unexpected event: {other:?}"),
    }
    assert_eq!(events_for_org(&db, org, "votes.tally.updated").await, 1);
}

#[tokio::test]
async fn duplicate_cast_is_conflict_and_tally_unchanged() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let citizen = seed_citizen(&db, org).await;
    let proposal = ProposalId::new();
    let svc = service(db.clone());

    svc.cast(org, proposal, citizen).await.expect("first cast");

    // A second cast by the same citizen for the same proposal is rejected.
    let err = svc.cast(org, proposal, citizen).await.unwrap_err();
    assert!(matches!(err, Error::Conflict(_)));

    // The tally and the linkage are unchanged — the conflict never double-counted.
    let count: i64 =
        sqlx::query_scalar("SELECT support_count FROM votes_vote_tally WHERE proposal_id = $1")
            .bind(proposal.as_uuid())
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(count, 1);
    let votes: i64 = sqlx::query_scalar("SELECT count(*) FROM votes_vote WHERE proposal_id = $1")
        .bind(proposal.as_uuid())
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(votes, 1);

    // No extra events were emitted by the failed second cast (still just the first cast's pair).
    assert_eq!(events_for_org(&db, org, "votes.cast").await, 1);
    assert_eq!(events_for_org(&db, org, "votes.tally.updated").await, 1);
}

#[tokio::test]
async fn official_tally_dto_carries_no_citizen_ids() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let a = seed_citizen(&db, org).await;
    let b = seed_citizen(&db, org).await;
    let proposal = ProposalId::new();
    let svc = service(db.clone());
    svc.cast(org, proposal, a).await.expect("cast a");
    svc.cast(org, proposal, b).await.expect("cast b");

    // The official-facing path returns the aggregate; mapping it to the wire DTO must not carry
    // any citizen linkage — proven over the serialized bytes a client would receive.
    let view = svc.tally(proposal).await.expect("tally");
    let dto = TallyDto::from(view);
    let json = serde_json::to_string(&dto).unwrap();
    assert!(!json.contains("citizen"));
    assert!(!json.contains(&a.as_uuid().to_string()));
    assert!(!json.contains(&b.as_uuid().to_string()));
    assert_eq!(dto.support_count, 2);
    assert_eq!(dto.proposal_id, proposal.as_uuid());
}

#[tokio::test]
async fn tally_equals_distinct_voter_count() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let proposal = ProposalId::new();
    let other_proposal = ProposalId::new();
    let svc = service(db.clone());

    // Three distinct citizens support `proposal`; one of them also supports another proposal.
    let mut voters = Vec::new();
    for _ in 0..3 {
        let c = seed_citizen(&db, org).await;
        svc.cast(org, proposal, c).await.expect("cast");
        voters.push(c);
    }
    svc.cast(org, other_proposal, voters[0])
        .await
        .expect("cast on other proposal");

    // The tally equals the distinct voter count for that proposal (not the global vote count).
    let distinct: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT citizen_id) FROM votes_vote WHERE proposal_id = $1",
    )
    .bind(proposal.as_uuid())
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(distinct, 3);

    let view = svc.tally(proposal).await.expect("tally");
    assert_eq!(view.support_count, u64::try_from(distinct).unwrap());
    assert_eq!(view.support_count, 3);

    // The other proposal is isolated at its own count.
    assert_eq!(svc.tally(other_proposal).await.unwrap().support_count, 1);
}

#[tokio::test]
async fn anonymous_caller_is_forbidden_by_handler() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let citizen = seed_citizen(&db, org).await;
    let proposal = ProposalId::new();

    // The authenticated caller is only anonymous-level: the mutating handler must reject before any
    // write (CRATE spec: anonymous cannot vote).
    let state = app_state(db.clone(), VerificationLevel::Anonymous);
    let req = CastVoteRequest {
        proposal_id: proposal.as_uuid(),
        org_id: None,
        citizen_id: None,
    };
    let caller = dsoc_app::CallerId { citizen, org };
    let response = dsoc_votes::http::cast(State(state), caller, Json(req)).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // Nothing was written — the gate fired before the service ran.
    let votes: i64 = sqlx::query_scalar("SELECT count(*) FROM votes_vote WHERE proposal_id = $1")
        .bind(proposal.as_uuid())
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(votes, 0);
    let tally: i64 =
        sqlx::query_scalar("SELECT count(*) FROM votes_vote_tally WHERE proposal_id = $1")
            .bind(proposal.as_uuid())
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(tally, 0);
}

/// Insere uma proposta minimal com `urgencia` explícito. `mandate` também precisa
/// existir por causa do FK. Retorna o proposal id.
async fn seed_proposal(db: &Db, org: OrgId, urgencia: &str) -> ProposalId {
    // Seed mandate primeiro (FK obrigatório).
    let mandate_id = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO mandate (id, org_id, office, display_name, public_email, sphere, is_candidate, created_at) \
         VALUES ($1, $2, 'DEPUTADO', 'Test', 'x@example.br', 'federal', false, $3)",
    )
    .bind(mandate_id)
    .bind(org.as_uuid())
    .bind(now())
    .execute(db)
    .await
    .expect("seed mandate");
    let proposal_id = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO proposal (id, org_id, mandate_id, title, body, threshold, urgencia, created_at) \
         VALUES ($1, $2, $3, 'T', 'B', 10, $4, $5)",
    )
    .bind(proposal_id)
    .bind(org.as_uuid())
    .bind(mandate_id)
    .bind(urgencia)
    .bind(now())
    .execute(db)
    .await
    .expect("seed proposal");
    ProposalId::from_uuid(proposal_id)
}

async fn set_titulo(db: &Db, citizen: CitizenId, status: Option<&str>) {
    sqlx::query("UPDATE citizen SET titulo_status = $2 WHERE id = $1")
        .bind(citizen.as_uuid())
        .bind(status)
        .execute(db)
        .await
        .expect("set titulo");
}

#[tokio::test]
async fn urgent_proposal_rejects_citizen_without_titulo() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let citizen = seed_citizen(&db, org).await; // titulo_status = NULL por default
    let proposal = seed_proposal(&db, org, "urgente").await;

    let err = service(db.clone())
        .cast(org, proposal, citizen)
        .await
        .unwrap_err();
    match err {
        Error::Forbidden(reason) => assert!(reason.contains("urgente"), "reason: {reason}"),
        other => panic!("esperava Forbidden, veio {other:?}"),
    }
}

#[tokio::test]
async fn urgent_proposal_accepts_citizen_with_titulo_validated() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let citizen = seed_citizen(&db, org).await;
    set_titulo(&db, citizen, Some("validated")).await;
    let proposal = seed_proposal(&db, org, "urgente").await;

    let receipt = service(db.clone())
        .cast(org, proposal, citizen)
        .await
        .expect("cast urgente com titulo validado");
    assert_eq!(receipt.support_count, 1);
}

#[tokio::test]
async fn comum_proposal_ignores_titulo_status() {
    // Backward-compat: propostas 'comum' (default) não passam pelo gate.
    let db = connect().await;
    let org = seed_org(&db).await;
    let citizen = seed_citizen(&db, org).await; // titulo = NULL
    let proposal = seed_proposal(&db, org, "comum").await;

    let receipt = service(db.clone())
        .cast(org, proposal, citizen)
        .await
        .expect("cast em comum sem titulo");
    assert_eq!(receipt.support_count, 1);
}

#[tokio::test]
async fn email_verified_caller_votes_via_handler() {
    let db = connect().await;
    let org = seed_org(&db).await;
    let citizen = seed_citizen(&db, org).await;
    let proposal = ProposalId::new();

    let state = app_state(db.clone(), VerificationLevel::Email);
    let req = CastVoteRequest {
        proposal_id: proposal.as_uuid(),
        org_id: None,
        citizen_id: None,
    };
    let caller = dsoc_app::CallerId { citizen, org };
    let response = dsoc_votes::http::cast(State(state), caller, Json(req)).await;
    assert_eq!(response.status(), StatusCode::CREATED);

    let count: i64 =
        sqlx::query_scalar("SELECT support_count FROM votes_vote_tally WHERE proposal_id = $1")
            .bind(proposal.as_uuid())
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(count, 1);
}

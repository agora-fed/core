//! Integration tests for `forums` directed dispatch (B1 — fusão Propor ≡ Fórum)
//! against a real PostgreSQL (TESTING.md: no mocked database).
//!
//! O coração testado é o **encaminhamento direcionado com integridade Tier 0**:
//! - tópico direcionado a mandato ALCANÇÁVEL → gera recibo ao e-mail real;
//! - tópico direcionado a mandato com placeholder `@parlamento.democracia.social.br`
//!   → **NÃO** gera recibo (nunca gritamos no vazio; o silêncio seria nosso);
//! - tópico SEM alvo → cai no contato curado da seção (comportamento atual);
//! - multi-alvo (1 alcançável + 1 placeholder) → exatamente 1 recibo, ao alcançável.
//!
//! Requer `DATABASE_URL` apontando pra um banco com a cadeia de migrations
//! aplicada (inclusive a 0666). Sem `DATABASE_URL`, cada teste faz SKIP (o gate
//! `cargo test -p dsoc-forums` segue verde), então rode com um DB descartável
//! migrado quando quiser exercer de fato o caminho.

// Test code: a failed `expect`/`panic` is a test failure, which is the desired behaviour.
#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use dsoc_core::clock::Clock;
use dsoc_core::ids::{CitizenId, MandateId, OrgId};
use dsoc_forums::{ForumService, NewTopic};
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

/// Connects to the test DB, or `None` to SKIP when `DATABASE_URL` is unset.
async fn pool_or_skip(test: &str) -> Option<PgPool> {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("SKIP {test}: DATABASE_URL não definido (rode com um DB descartável migrado)");
        return None;
    };
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("connect to PostgreSQL");
    Some(pool)
}

fn service(db: PgPool) -> ForumService {
    ForumService::new(db, fixed_clock())
}

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

/// Seed a mandate with an explicit public email (real gabinete OR placeholder).
async fn seed_mandate(db: &PgPool, org: OrgId, email: &str) -> MandateId {
    let mandate = MandateId::new();
    sqlx::query(
        "INSERT INTO mandate (id, org_id, office, display_name, public_email, created_at) \
         VALUES ($1, $2, 'vereador', 'Fulana de Tal', $3, now())",
    )
    .bind(mandate.as_uuid())
    .bind(org.as_uuid())
    .bind(email)
    .execute(db)
    .await
    .expect("seed mandate");
    mandate
}

/// Seed a bare forum (no esfera → threshold cai no piso 10) with an optional
/// curated contact email; returns its path.
async fn seed_forum(db: &PgPool, org: OrgId, contact_email: Option<&str>) -> String {
    let id = Uuid::now_v7();
    let path = format!("teste-{}", id.as_simple());
    sqlx::query(
        "INSERT INTO forum (id, org_id, slug, full_path, name, kind, contact_email, created_at) \
         VALUES ($1, $2, $3, $3, 'Fórum de Teste', 'comunitario', $4, now())",
    )
    .bind(id)
    .bind(org.as_uuid())
    .bind(&path)
    .bind(contact_email)
    .execute(db)
    .await
    .expect("seed forum");
    path
}

/// Push the topic's score across the proportional floor (10) with `n` distinct
/// favor voters (each +1), returning after the crossing vote.
async fn cross_threshold(svc: &ForumService, db: &PgPool, org: OrgId, topic_id: Uuid, n: usize) {
    for _ in 0..n {
        let voter = seed_citizen(db, org).await;
        svc.vote(topic_id, voter, dsoc_forums::domain::Stance::Favor)
            .await
            .expect("vote favor");
    }
}

/// Dispatch rows for a topic: (contact_email, mandate_id).
async fn dispatches(db: &PgPool, topic_id: Uuid) -> Vec<(String, Option<Uuid>)> {
    sqlx::query_as(
        "SELECT contact_email, mandate_id FROM forum_dispatch \
         WHERE topic_id = $1 ORDER BY contact_email",
    )
    .bind(topic_id)
    .fetch_all(db)
    .await
    .expect("query dispatches")
}

async fn next_threshold_idx(db: &PgPool, topic_id: Uuid) -> i32 {
    let (idx,): (i32,) = sqlx::query_as("SELECT next_threshold_idx FROM forum_topic WHERE id = $1")
        .bind(topic_id)
        .fetch_one(db)
        .await
        .expect("query idx");
    idx
}

const FLOOR: usize = 12; // piso proporcional é 10; 12 votos cruzam com folga

#[tokio::test]
async fn directed_reachable_mandate_dispatches_to_real_email() {
    let Some(db) = pool_or_skip("directed_reachable").await else {
        return;
    };
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;
    let gabinete = seed_mandate(&db, org, "gabinete@example.org").await;
    let path = seed_forum(&db, org, None).await;

    let new = NewTopic::validate("Ciclovia já", "Precisamos de ciclovia no bairro.").unwrap();
    let topic = svc
        .create_topic(org, &path, author, &new, &[gabinete.as_uuid()])
        .await
        .expect("create directed topic");

    cross_threshold(&svc, &db, org, topic.id, FLOOR).await;

    let d = dispatches(&db, topic.id).await;
    assert_eq!(d.len(), 1, "um recibo ao alvo alcançável");
    assert_eq!(d[0].0, "gabinete@example.org");
    assert_eq!(d[0].1, Some(gabinete.as_uuid()));
    assert_eq!(next_threshold_idx(&db, topic.id).await, 1, "escalou 1x");
}

#[tokio::test]
async fn directed_placeholder_mandate_never_dispatches() {
    let Some(db) = pool_or_skip("directed_placeholder").await else {
        return;
    };
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;
    // Placeholder da plataforma — canal MORTO; Tier 0 diz: nunca entregar/carimbar.
    let unreachable = seed_mandate(&db, org, "gab@parlamento.democracia.social.br").await;
    let path = seed_forum(&db, org, None).await;

    let new = NewTopic::validate("Praça viva", "Reforma da praça central.").unwrap();
    let topic = svc
        .create_topic(org, &path, author, &new, &[unreachable.as_uuid()])
        .await
        .expect("create directed topic");

    cross_threshold(&svc, &db, org, topic.id, FLOOR).await;

    let d = dispatches(&db, topic.id).await;
    assert!(d.is_empty(), "Tier 0: NENHUM recibo a alvo inalcançável");
    assert_eq!(
        next_threshold_idx(&db, topic.id).await,
        0,
        "sem canal alcançável → fica PENDENTE (idx não avança)"
    );
}

#[tokio::test]
async fn untargeted_topic_falls_back_to_section_contact() {
    let Some(db) = pool_or_skip("untargeted_section").await else {
        return;
    };
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;
    let path = seed_forum(&db, org, Some("secretaria@cidade.gov.br")).await;

    let new = NewTopic::validate("Iluminação", "Postes queimados na rua X.").unwrap();
    let topic = svc
        .create_topic(org, &path, author, &new, &[])
        .await
        .expect("create untargeted topic");

    cross_threshold(&svc, &db, org, topic.id, FLOOR).await;

    let d = dispatches(&db, topic.id).await;
    assert_eq!(d.len(), 1, "um recibo ao contato curado da seção");
    assert_eq!(d[0].0, "secretaria@cidade.gov.br");
    assert_eq!(d[0].1, None, "seção = mandate_id NULL");
}

#[tokio::test]
async fn multi_target_dispatches_only_to_reachable() {
    let Some(db) = pool_or_skip("multi_target").await else {
        return;
    };
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;
    let reachable = seed_mandate(&db, org, "real@example.org").await;
    let placeholder = seed_mandate(&db, org, "dead@parlamento.democracia.social.br").await;
    let path = seed_forum(&db, org, None).await;

    let new = NewTopic::validate("Merenda", "Melhorar a merenda escolar.").unwrap();
    let topic = svc
        .create_topic(
            org,
            &path,
            author,
            &new,
            &[reachable.as_uuid(), placeholder.as_uuid()],
        )
        .await
        .expect("create multi-target topic");

    cross_threshold(&svc, &db, org, topic.id, FLOOR).await;

    let d = dispatches(&db, topic.id).await;
    assert_eq!(d.len(), 1, "exatamente 1 recibo (só o alcançável)");
    assert_eq!(d[0].0, "real@example.org");
    assert_eq!(d[0].1, Some(reachable.as_uuid()));
    assert_eq!(next_threshold_idx(&db, topic.id).await, 1, "escalou 1x");
}

#[tokio::test]
async fn create_topic_rejects_nonexistent_target() {
    let Some(db) = pool_or_skip("nonexistent_target").await else {
        return;
    };
    let svc = service(db.clone());
    let org = seed_org(&db).await;
    let author = seed_citizen(&db, org).await;
    let path = seed_forum(&db, org, None).await;

    let new = NewTopic::validate("Alvo fantasma", "Direcionado a mandato inexistente.").unwrap();
    let err = svc
        .create_topic(org, &path, author, &new, &[Uuid::now_v7()])
        .await
        .expect_err("mandato inexistente deve falhar");
    assert!(
        matches!(err, dsoc_core::Error::Validation(_)),
        "alvo inexistente → Validation, got {err:?}"
    );
}

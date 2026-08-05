//! Integration tests for `forums` directed dispatch (B1 — the Propose ≡ Forum merge)
//! against a real PostgreSQL (TESTING.md: no mocked database).
//!
//! The heart under test is **directed dispatch with Tier 0 integrity**:
//! - a topic directed at a REACHABLE mandate → produces a receipt to the real e-mail;
//! - a topic directed at a mandate with the `@parlamento.democracia.social.br` placeholder
//!   → produces **NO** receipt (we never shout into the void; the silence would be ours);
//! - a topic with NO target → falls back to the section's curated contact (current behaviour);
//! - multi-target (1 reachable + 1 placeholder) → exactly 1 receipt, to the reachable one.
//!
//! Requires `DATABASE_URL` pointing at a database with the migration chain applied
//! (0666 included). Without `DATABASE_URL` every test SKIPs (so the
//! `cargo test -p dsoc-forums` gate stays green) — run it against a disposable,
//! migrated DB when you want the path genuinely exercised.

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
    // Platform placeholder — a DEAD channel; Tier 0 says: never deliver, never stamp.
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

/// A unique municipio name so each transparency test is isolated in the shared DB
/// (`municipal_transparency` matches on `(uf, municipio)` across all orgs).
fn unique_municipio() -> String {
    format!("Cidade {}", Uuid::now_v7().as_simple())
}

/// Seed a MUNICIPAL vereador mandate (sphere/house/uf/municipio explicit) with an
/// explicit public email — the real signal the transparency banner now derives from.
async fn seed_municipal_mandate(
    db: &PgPool,
    org: OrgId,
    uf: &str,
    municipio: &str,
    email: &str,
) -> MandateId {
    let mandate = MandateId::new();
    sqlx::query(
        "INSERT INTO mandate \
           (id, org_id, office, display_name, public_email, sphere, house, uf, municipio, created_at) \
         VALUES ($1, $2, 'vereador', 'Vereador Teste', $3, 'municipal', 'camara_municipal', $4, $5, now())",
    )
    .bind(mandate.as_uuid())
    .bind(org.as_uuid())
    .bind(email)
    .bind(uf)
    .bind(municipio)
    .execute(db)
    .await
    .expect("seed municipal mandate");
    mandate
}

/// Seed a `civic_source` catalog row (the site oficial signal → `official_url`).
async fn seed_civic_source(db: &PgPool, uf: &str, municipio: &str, base_url: Option<&str>) {
    sqlx::query(
        "INSERT INTO civic_source (uf, municipio, platform, base_url, probe_status, created_at) \
         VALUES ($1, $2, 'wordpress', $3, 'ok', now())",
    )
    .bind(uf)
    .bind(municipio)
    .bind(base_url)
    .execute(db)
    .await
    .expect("seed civic_source");
}

/// `plena` derives from the REAL signal: ≥1 council member with a reachable e-mail —
/// even WITHOUT a `civic_source` row (a stale `probe_status` no longer downgrades the council).
#[tokio::test]
async fn municipal_transparency_plena_when_reachable_gabinete() {
    let Some(db) = pool_or_skip("transp_plena").await else {
        return;
    };
    let org = seed_org(&db).await;
    let municipio = unique_municipio();
    seed_municipal_mandate(&db, org, "SP", &municipio, "vereador@camara.sp.gov.br").await;

    // A lowercase uf proves the case-insensitive match (upper() on both sides).
    let (status, url) = dsoc_forums::queries::municipal_transparency(&db, "sp", &municipio)
        .await
        .expect("query transparency")
        .expect("always Some");
    assert_eq!(status, "plena", "gabinete alcançável ⇒ plena");
    assert_eq!(url, None, "sem civic_source ⇒ sem site oficial");
}

/// The platform placeholder does NOT count as a connected office: with a catalogued
/// official site but only a placeholder e-mail ⇒ `parcial` (fixes the false `plena`).
#[tokio::test]
async fn municipal_transparency_parcial_when_only_placeholder_gabinete() {
    let Some(db) = pool_or_skip("transp_parcial").await else {
        return;
    };
    let org = seed_org(&db).await;
    let municipio = unique_municipio();
    seed_municipal_mandate(
        &db,
        org,
        "SP",
        &municipio,
        "morto@parlamento.democracia.social.br",
    )
    .await;
    seed_civic_source(&db, "SP", &municipio, Some("https://camara.example.gov.br")).await;

    let (status, url) = dsoc_forums::queries::municipal_transparency(&db, "SP", &municipio)
        .await
        .expect("query transparency")
        .expect("always Some");
    assert_eq!(status, "parcial", "só placeholder + site ⇒ parcial");
    assert_eq!(url.as_deref(), Some("https://camara.example.gov.br"));
}

/// Neither a connected office nor a catalogued official site ⇒ `ausente`.
#[tokio::test]
async fn municipal_transparency_ausente_when_nothing() {
    let Some(db) = pool_or_skip("transp_ausente").await else {
        return;
    };
    let municipio = unique_municipio();
    let (status, url) = dsoc_forums::queries::municipal_transparency(&db, "SP", &municipio)
        .await
        .expect("query transparency")
        .expect("always Some");
    assert_eq!(
        status, "ausente",
        "nada catalogado, nenhum gabinete ⇒ ausente"
    );
    assert_eq!(url, None);
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

//! # Tag-a-representative (migration 0676, issue agora-fed/core#3).
//!
//! A citizen marks ONE mandate per forum topic as the representative who
//! should care about that cause. Once a day the worker compiles each
//! mandate's new tags and sends ONE consolidated e-mail to the mandate's
//! public address — until the official onboards (identity binding) and
//! receives it in-platform instead.
//!
//! Routes (merged into the gateway's `/api/v1` group via [`routes`]):
//! * `GET    /topics/{id}/representatives` — PUBLIC aggregate per mandate
//!   (never lists citizens — LGPD/ADR-0005 posture) + `mine` when authed.
//! * `POST   /topics/{id}/representatives` — auth; body `{mandate_id}`;
//!   ADDS a pick (a citizen may mark several mandates, capped at
//!   [`MAX_TAGS_PER_CITIZEN`]; duplicates are idempotent).
//! * `DELETE /topics/{id}/representatives/{mandate_id}` — auth; removes ONE
//!   of the caller's picks.
//!
//! Daily sweep: [`daily_alert_sweep`] — claims `(mandate, day)` in
//! `mandate_alert_delivery` (idempotent), then mails the consolidated alert.
//! Federated interactions never reach here (tags require a local session),
//! and the mandate gets at most ONE e-mail per day.

use axum::extract::{Json, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use chrono::{Duration, Utc};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::proposal_delivery::{send_email, smtp_from_env};

const DEFAULT_ORG_UUID: Uuid = uuid::uuid!("11111111-1111-1111-1111-111111111111");
/// Public aggregate cap — the page shows the most-tagged mandates.
const AGGREGATE_LIMIT: i64 = 20;
/// Max mandates alerted per sweep tick (backpressure; the claim makes the
/// remainder pick up on the next tick).
const SWEEP_BATCH: i64 = 50;
/// Cap of representatives one citizen may mark on one topic (0677): enough
/// for a caucus, small enough that "tag everyone" cannot dilute the ranking.
pub const MAX_TAGS_PER_CITIZEN: i64 = 5;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route(
            "/topics/{id}/representatives",
            get(list_representatives).post(tag_representative),
        )
        .route(
            "/topics/{id}/representatives/{mandate_id}",
            axum::routing::delete(untag_representative),
        )
        .with_state(state)
}

// ---------------------------------------------------------------------------
// DTOs (English contract, ADR-0013)
// ---------------------------------------------------------------------------

/// Aggregate view of one tagged mandate on a topic. NEVER carries citizens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicRepresentativeDto {
    pub mandate_id: Uuid,
    pub display_name: String,
    pub office: String,
    pub party: Option<String>,
    pub state: Option<String>,
    pub avatar_url: Option<String>,
    pub tag_count: i64,
}

/// `GET` response: ranked aggregates + the caller's own picks (when authed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicRepresentativesDto {
    pub representatives: Vec<TopicRepresentativeDto>,
    pub total_tags: i64,
    /// The mandates the CALLER tagged on this topic (empty when anonymous or untagged).
    pub mine: Vec<Uuid>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TagInput {
    pub mandate_id: Uuid,
}

// ---------------------------------------------------------------------------
// Helpers (same conventions as admin_content.rs)
// ---------------------------------------------------------------------------

fn caller_citizen(headers: &HeaderMap) -> Option<Uuid> {
    headers
        .get("x-dsoc-citizen-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
}

fn caller_org(headers: &HeaderMap) -> Uuid {
    headers
        .get("x-dsoc-org-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_ORG_UUID)
}

fn fail(status: StatusCode, code: &str, msg: &str) -> Response {
    (status, Json(ApiResponse::<()>::fail(code, msg))).into_response()
}

/// The topic must exist, be visible, and belong to the caller's org.
async fn topic_visible(db: &PgPool, org: Uuid, topic: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>(
        r"SELECT EXISTS(
            SELECT 1 FROM forum_topic t
              JOIN forum f ON f.id = t.forum_id
             WHERE t.id = $1 AND f.org_id = $2
               AND t.hidden_at IS NULL AND f.hidden_at IS NULL)",
    )
    .bind(topic)
    .bind(org)
    .fetch_one(db)
    .await
    .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /topics/{id}/representatives` — public ranked aggregate.
async fn list_representatives(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(topic_id): Path<Uuid>,
) -> Response {
    let org = caller_org(&headers);
    if !topic_visible(&state.db, org, topic_id).await {
        return fail(StatusCode::NOT_FOUND, "not_found", "Tópico não encontrado.");
    }
    type AggRow = (
        Uuid,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        i64,
    );
    let rows: Vec<AggRow> = sqlx::query_as(
        r"SELECT m.id, m.display_name, m.office, m.party, m.uf, m.avatar_object_key,
                 count(*) AS tag_count
            FROM topic_representative_tag t
            JOIN mandate m ON m.id = t.mandate_id
           WHERE t.org_id = $1 AND t.topic_id = $2 AND m.hidden_at IS NULL
           GROUP BY m.id, m.display_name, m.office, m.party, m.uf, m.avatar_object_key
           ORDER BY tag_count DESC, m.display_name ASC
           LIMIT $3",
    )
    .bind(org)
    .bind(topic_id)
    .bind(AGGREGATE_LIMIT)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let total_tags: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM topic_representative_tag WHERE org_id = $1 AND topic_id = $2",
    )
    .bind(org)
    .bind(topic_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    let mine = match caller_citizen(&headers) {
        Some(citizen) => sqlx::query_scalar::<_, Uuid>(
            "SELECT mandate_id FROM topic_representative_tag \
             WHERE org_id = $1 AND topic_id = $2 AND citizen_id = $3 \
             ORDER BY created_at ASC",
        )
        .bind(org)
        .bind(topic_id)
        .bind(citizen)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default(),
        None => Vec::new(),
    };

    let representatives = rows
        .into_iter()
        .map(
            |(mandate_id, display_name, office, party, uf, avatar_key, tag_count)| {
                TopicRepresentativeDto {
                    mandate_id,
                    display_name,
                    office,
                    party,
                    state: uf.map(|u| u.trim().to_uppercase()),
                    avatar_url: resolve_avatar(avatar_key.as_deref()),
                    tag_count,
                }
            },
        )
        .collect();

    (
        StatusCode::OK,
        Json(ApiResponse::ok(TopicRepresentativesDto {
            representatives,
            total_tags,
            mine,
        })),
    )
        .into_response()
}

/// Resolve an avatar object key to a public URL (same convention as
/// `politicos_ext`): absolute URLs pass through, keys go under MEDIA_BASE_URL.
fn resolve_avatar(object_key: Option<&str>) -> Option<String> {
    let key = object_key?;
    if key.starts_with("http://") || key.starts_with("https://") || key.starts_with('/') {
        return Some(key.to_owned());
    }
    let base = std::env::var("MEDIA_BASE_URL").unwrap_or_else(|_| "/media".to_owned());
    Some(format!("{}/{}", base.trim_end_matches('/'), key))
}

/// `POST /topics/{id}/representatives` — upsert the caller's pick.
async fn tag_representative(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(topic_id): Path<Uuid>,
    Json(input): Json<TagInput>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return fail(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Autenticação necessária.",
        );
    };
    let org = caller_org(&headers);
    if !topic_visible(&state.db, org, topic_id).await {
        return fail(StatusCode::NOT_FOUND, "not_found", "Tópico não encontrado.");
    }
    let mandate_ok: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM mandate \
          WHERE id = $1 AND org_id = $2 AND hidden_at IS NULL)",
    )
    .bind(input.mandate_id)
    .bind(org)
    .fetch_one(&state.db)
    .await
    .unwrap_or(false);
    if !mandate_ok {
        return fail(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "Mandato inválido.",
        );
    }
    // Cap per citizen per topic (0677): additive picks, bounded.
    let current: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM topic_representative_tag \
         WHERE org_id = $1 AND topic_id = $2 AND citizen_id = $3",
    )
    .bind(org)
    .bind(topic_id)
    .bind(citizen)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);
    if current >= MAX_TAGS_PER_CITIZEN {
        return fail(
            StatusCode::UNPROCESSABLE_ENTITY,
            "limit",
            "Você já marcou o máximo de representantes nesta pauta.",
        );
    }
    let result = sqlx::query(
        r"INSERT INTO topic_representative_tag (org_id, topic_id, mandate_id, citizen_id)
          VALUES ($1, $2, $3, $4)
          ON CONFLICT (org_id, topic_id, citizen_id, mandate_id) DO NOTHING",
    )
    .bind(org)
    .bind(topic_id)
    .bind(input.mandate_id)
    .bind(citizen)
    .execute(&state.db)
    .await;
    match result {
        Ok(_) => (StatusCode::OK, Json(ApiResponse::ok(()))).into_response(),
        Err(err) => {
            tracing::error!(?err, "representatives: tag upsert failed");
            fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage",
                "Erro interno.",
            )
        }
    }
}

/// `DELETE /topics/{id}/representatives/{mandate_id}` — remove ONE of the
/// caller's picks.
async fn untag_representative(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((topic_id, mandate_id)): Path<(Uuid, Uuid)>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return fail(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Autenticação necessária.",
        );
    };
    let _ = sqlx::query(
        "DELETE FROM topic_representative_tag \
         WHERE org_id = $1 AND topic_id = $2 AND citizen_id = $3 AND mandate_id = $4",
    )
    .bind(caller_org(&headers))
    .bind(topic_id)
    .bind(citizen)
    .bind(mandate_id)
    .execute(&state.db)
    .await;
    (StatusCode::OK, Json(ApiResponse::ok(()))).into_response()
}

// ---------------------------------------------------------------------------
// Daily consolidated alert (worker sweep)
// ---------------------------------------------------------------------------

/// Render the daily consolidated alert (pure — unit-tested). Each topic line
/// carries the SIGNED public placar (favor − contra) and the direction the
/// population is asking for: negative placar = "LUTE CONTRA" (the citizens
/// tagged this mandate to fight the proposal), positive = "DEFENDA",
/// zero = still in dispute. User-facing copy stays in the installation's
/// language (pt-BR).
fn render_alert_email(
    display_name: &str,
    day: chrono::NaiveDate,
    origin: &str,
    tag_count: i64,
    topics: &[(Uuid, String, i64, i64)],
) -> (String, String) {
    let stance = |score: i64| -> String {
        if score < 0 {
            format!(
                "placar {score} — a população está majoritariamente CONTRA: \
                 ela pede que você LUTE CONTRA esta pauta"
            )
        } else if score > 0 {
            format!("placar +{score} — a população APOIA: ela pede que você DEFENDA esta pauta")
        } else {
            "placar 0 — pauta em disputa; a população pede sua posição".to_owned()
        }
    };
    let lines: String = topics
        .iter()
        .map(|(topic_id, title, c, score)| {
            format!(
                "• {title}\n  {c} cidadã(o)s marcaram você · {}\n  {origin}/f/topico/{topic_id}\n",
                stance(*score)
            )
        })
        .collect();
    let subject = format!(
        "[DemocraciaBR] {tag_count} cidadã(o)s marcaram você em causas públicas — {}",
        day.format("%d/%m/%Y")
    );
    let body = format!(
        "Prezada(o) {display_name},\n\n\
         Nas últimas 24 horas, {tag_count} cidadã(o)s verificados marcaram o seu mandato\n\
         como quem deve representá-los nas causas abaixo. Atenção ao PLACAR de cada\n\
         pauta: ele diz de que lado a população está — marcar você numa pauta de placar\n\
         negativo é um pedido para BARRÁ-LA, não para levá-la adiante.\n\n\
         {lines}\n\
         Este é um resumo automático DIÁRIO e consolidado — no máximo um e-mail por dia.\n\
         Os números são agregados: a plataforma não expõe cidadãos individualmente.\n\n\
         Para responder às causas, reivindique seu mandato e passe a receber tudo\n\
         dentro da plataforma: {origin}/politicos\n\n\
         — DemocraciaBR (sistema automático, registro público)\n"
    );
    (subject, body)
}

/// Compile yesterday's tags per mandate and send ONE consolidated e-mail to
/// each mandate's public address. Idempotent: `(mandate, day)` is claimed in
/// `mandate_alert_delivery` before any send; ticks can run as often as the
/// worker likes. Onboarded mandates (identity binding) are skipped — they
/// get the in-platform path instead (follow-up in issue #3).
pub async fn daily_alert_sweep(db: &PgPool, public_origin: &str) {
    let day = (Utc::now() - Duration::days(1)).date_naive();
    let origin = public_origin.trim_end_matches('/');

    // Mandates with new tags on `day`, not yet claimed.
    let candidates: Vec<(Uuid, i64)> = match sqlx::query_as(
        r"SELECT t.mandate_id, count(*) AS c
            FROM topic_representative_tag t
           WHERE t.created_at >= $1::date AND t.created_at < ($1::date + interval '1 day')
             AND NOT EXISTS (SELECT 1 FROM mandate_alert_delivery d
                              WHERE d.mandate_id = t.mandate_id AND d.day = $1)
           GROUP BY t.mandate_id
           ORDER BY c DESC
           LIMIT $2",
    )
    .bind(day)
    .bind(SWEEP_BATCH)
    .fetch_all(db)
    .await
    {
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!(?err, "representatives sweep: candidate lookup failed");
            return;
        }
    };
    if candidates.is_empty() {
        return;
    }
    let smtp = smtp_from_env();

    for (mandate_id, tag_count) in candidates {
        // Claim first — concurrent ticks and reruns become no-ops.
        let claimed = sqlx::query(
            "INSERT INTO mandate_alert_delivery (mandate_id, day, tag_count) \
             VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
        )
        .bind(mandate_id)
        .bind(day)
        .bind(tag_count)
        .execute(db)
        .await
        .map(|r| r.rows_affected() > 0)
        .unwrap_or(false);
        if !claimed {
            continue;
        }

        let Ok(Some((display_name, public_email, onboarded))) =
            sqlx::query_as::<_, (String, String, bool)>(
                r"SELECT m.display_name, m.public_email,
                         EXISTS(SELECT 1 FROM mandate_identity_binding b
                                 WHERE b.mandate_id = m.id) AS onboarded
                    FROM mandate m WHERE m.id = $1",
            )
            .bind(mandate_id)
            .fetch_optional(db)
            .await
        else {
            continue;
        };
        if onboarded {
            // In-platform path (mandate claimed): no unsolicited e-mail.
            tracing::info!(%mandate_id, "representatives sweep: onboarded, e-mail skipped");
            let _ = sqlx::query(
                "UPDATE mandate_alert_delivery SET sent_at = now() \
                 WHERE mandate_id = $1 AND day = $2",
            )
            .bind(mandate_id)
            .bind(day)
            .execute(db)
            .await;
            continue;
        }

        // The topics behind the tags (title + per-topic count + SIGNED score),
        // most tagged first. The score (favor − contra, ADR-0019) tells the
        // mandate WHICH SIDE the population is on: a negative placar means
        // "fight AGAINST this", not "push it forward".
        let topics: Vec<(Uuid, String, i64, i64)> = sqlx::query_as(
            r"SELECT t.topic_id, ft.title, count(*) AS c, ft.score
                FROM topic_representative_tag t
                JOIN forum_topic ft ON ft.id = t.topic_id
               WHERE t.mandate_id = $1
                 AND t.created_at >= $2::date AND t.created_at < ($2::date + interval '1 day')
               GROUP BY t.topic_id, ft.title, ft.score
               ORDER BY c DESC
               LIMIT 10",
        )
        .bind(mandate_id)
        .bind(day)
        .fetch_all(db)
        .await
        .unwrap_or_default();

        let (subject, body) = render_alert_email(&display_name, day, origin, tag_count, &topics);

        let sent = match &smtp {
            Some(cfg) => match send_email(cfg, &public_email, &subject, &body).await {
                Ok(()) => true,
                Err(err) => {
                    tracing::warn!(?err, to = %public_email, "representatives sweep: SMTP failed");
                    false
                }
            },
            None => {
                tracing::info!(target: "representatives_sweep", to = %public_email,
                    subject = %subject, "DEV mode: e-mail logged, not sent");
                true
            }
        };
        if sent {
            let _ = sqlx::query(
                "UPDATE mandate_alert_delivery SET sent_at = now() \
                 WHERE mandate_id = $1 AND day = $2",
            )
            .bind(mandate_id)
            .bind(day)
            .execute(db)
            .await;
        } else {
            // Release the claim so the next tick retries the send.
            let _ = sqlx::query(
                "DELETE FROM mandate_alert_delivery \
                 WHERE mandate_id = $1 AND day = $2 AND sent_at IS NULL",
            )
            .bind(mandate_id)
            .bind(day)
            .execute(db)
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day() -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(2026, 8, 5).unwrap()
    }

    #[test]
    fn negative_placar_frames_the_alert_as_fight_against() {
        let topic = Uuid::nil();
        let (subject, body) = render_alert_email(
            "Dep. Teste",
            day(),
            "https://example.org",
            42,
            &[(topic, "Isenção de IR para militares".into(), 42, -128)],
        );
        assert!(subject.contains("42 cidadã(o)s"));
        assert!(body.contains("placar -128"), "body={body}");
        assert!(
            body.contains("LUTE CONTRA"),
            "negative score must ask to fight it"
        );
        assert!(body.contains("majoritariamente CONTRA"));
        assert!(!body.contains("DEFENDA esta pauta"));
    }

    #[test]
    fn positive_placar_frames_the_alert_as_defend() {
        let (_, body) = render_alert_email(
            "Dep. Teste",
            day(),
            "https://example.org",
            7,
            &[(Uuid::nil(), "Passe livre estudantil".into(), 7, 315)],
        );
        assert!(body.contains("placar +315"), "body={body}");
        assert!(body.contains("DEFENDA esta pauta"));
        assert!(!body.contains("LUTE CONTRA"));
    }

    #[test]
    fn zero_placar_is_in_dispute_and_mixed_topics_keep_their_own_framing() {
        let (_, body) = render_alert_email(
            "Dep. Teste",
            day(),
            "https://example.org",
            10,
            &[
                (Uuid::nil(), "Pauta empatada".into(), 4, 0),
                (Uuid::nil(), "Pauta apoiada".into(), 3, 9),
                (Uuid::nil(), "Pauta rejeitada".into(), 3, -9),
            ],
        );
        assert!(body.contains("pauta em disputa"));
        assert!(body.contains("DEFENDA esta pauta"));
        assert!(body.contains("LUTE CONTRA"));
        // The intro always explains the placar semantics to the mandate.
        assert!(body.contains("pedido para BARRÁ-LA"));
    }
}

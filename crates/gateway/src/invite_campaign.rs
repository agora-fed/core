//! # Invitation campaign to the cabinets (0.34.0).
//!
//! The plan's strategic bottleneck is adoption: the accountability machine only
//! generates narrative when offices answer — and they only answer after they
//! join. This module turns the individual invitation (`POST
//! /mandates/{id}/invites`, dsoc-auth) into a CAMPAIGN the admin can operate:
//!
//! - `GET  /admin/invite-campaign/overview`   — the funnel: eligible, invited,
//! - `GET  /admin/invite-campaign/overview`   — the funnel: eligible, invited,
//!   pending, accepted, expired (sphere/house/state/party filters).
//! - `POST /admin/invite-campaign/send-batch` — sends a BATCH (1–50) to the
//!   (same guards: admin-only, hashed token, TTL, templated e-mail).
//! - `GET  /admin/invite-campaign/invites`    — tracking by status.
//!
//! The button stays with the human: nothing here fires on its own — the batch goes out when
//! the admin clicks, at the size the admin chose.

use axum::extract::{Json, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, Utc};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use dsoc_core::ids::{CitizenId, OrgId};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

const MAX_BATCH: i64 = 50;
const DEFAULT_BATCH: i64 = 20;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/admin/invite-campaign/overview", get(overview))
        .route("/admin/invite-campaign/send-batch", post(send_batch))
        .route("/admin/invite-campaign/invites", get(list_invites))
        .with_state(state)
}

// The same guard as the other admin modules (email_templates, admin_users).
/// Org-scoped admin gate — delegates to the single implementation in
/// [`crate::authz_ext::require_org_admin`] (issue #8). This module used to carry
/// its own copy that omitted `org_id`, so an owner of ANY org passed it.
async fn require_admin(headers: &HeaderMap, db: &PgPool) -> Result<Uuid, Response> {
    crate::authz_ext::require_org_admin(db, headers)
        .await
        .map(|a| a.citizen)
}

#[derive(Debug, Deserialize)]
struct CampaignFilter {
    /// Default 'federal' — the electoral-window campaign targets the 594.
    sphere: Option<String>,
    house: Option<String>,
    uf: Option<String>,
    party: Option<String>,
}

/// Shared WHERE fragment + binds in the order sphere, house, state, party.
/// Filters are exact equality; `None` becomes TRUE via `$n IS NULL`.
const FILTER_SQL: &str = r"
      ($1::text IS NULL OR m.sphere = $1)
  AND ($2::text IS NULL OR m.house  = $2)
  AND ($3::text IS NULL OR m.uf     = $3)
  AND ($4::text IS NULL OR m.party  = $4)";

fn sphere_or_default(f: &CampaignFilter) -> Option<String> {
    f.sphere
        .clone()
        .or_else(|| Some("federal".to_owned()))
        .filter(|s| s != "todas")
}

#[derive(Debug, Serialize)]
struct OverviewDto {
    total: i64,
    with_email: i64,
    bound: i64,
    invite_pending: i64,
    invite_accepted: i64,
    invite_expired: i64,
    eligible_now: i64,
}

/// `GET /admin/invite-campaign/overview` — the funnel in a single SELECT.
async fn overview(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(filter): Query<CampaignFilter>,
) -> Response {
    if let Err(resp) = require_admin(&headers, &state.db).await {
        return resp;
    }
    let sql = format!(
        r"SELECT
            count(*),
            count(*) FILTER (WHERE has_email),
            count(*) FILTER (WHERE bound),
            count(*) FILTER (WHERE pending),
            count(*) FILTER (WHERE accepted),
            count(*) FILTER (WHERE expired),
            count(*) FILTER (WHERE has_email AND NOT bound AND NOT pending AND NOT accepted)
          FROM (
            SELECT
              m.public_email IS NOT NULL AND position('@' in COALESCE(m.public_email,'')) > 1
                AND m.public_email NOT ILIKE '%@parlamento.democracia.social.br' AS has_email,
              EXISTS (SELECT 1 FROM mandate_identity_binding b WHERE b.mandate_id = m.id) AS bound,
              EXISTS (SELECT 1 FROM mandate_invite i WHERE i.mandate_id = m.id
                        AND i.accepted_at IS NULL AND i.revoked_at IS NULL
                        AND i.expires_at > now()) AS pending,
              EXISTS (SELECT 1 FROM mandate_invite i WHERE i.mandate_id = m.id
                        AND i.accepted_at IS NOT NULL) AS accepted,
              EXISTS (SELECT 1 FROM mandate_invite i WHERE i.mandate_id = m.id
                        AND i.accepted_at IS NULL AND i.revoked_at IS NULL
                        AND i.expires_at <= now()) AS expired
            FROM mandate m
            WHERE {FILTER_SQL}
          ) t",
    );
    let row: Result<(i64, i64, i64, i64, i64, i64, i64), _> = sqlx::query_as(&sql)
        .bind(sphere_or_default(&filter))
        .bind(&filter.house)
        .bind(&filter.uf)
        .bind(&filter.party)
        .fetch_one(&state.db)
        .await;
    match row {
        Ok((
            total,
            with_email,
            bound,
            invite_pending,
            invite_accepted,
            invite_expired,
            eligible_now,
        )) => (
            StatusCode::OK,
            Json(ApiResponse::ok(OverviewDto {
                total,
                with_email,
                bound,
                invite_pending,
                invite_accepted,
                invite_expired,
                eligible_now,
            })),
        )
            .into_response(),
        Err(err) => storage_err(err),
    }
}

#[derive(Debug, Deserialize)]
struct SendBatchBody {
    #[serde(flatten)]
    filter: CampaignFilter,
    /// Batch size (1–50, default 20). Small ON PURPOSE: a campaign
    /// goes in controlled waves, not in a blast.
    limit: Option<i64>,
}

#[derive(Debug, Serialize)]
struct BatchItemDto {
    mandate_id: Uuid,
    display_name: String,
    email: String,
    ok: bool,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct BatchResultDto {
    attempted: usize,
    sent: usize,
    failed: usize,
    items: Vec<BatchItemDto>,
}

/// `POST /admin/invite-campaign/send-batch` — takes the next N eligible ones
/// (never invited, or with an expired invitation) and fires each one's real
/// invitation, sequentially. An error on one item does not break the batch.
async fn send_batch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SendBatchBody>,
) -> Response {
    let admin = match require_admin(&headers, &state.db).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let limit = body.limit.unwrap_or(DEFAULT_BATCH).clamp(1, MAX_BATCH);
    let sql = format!(
        r"SELECT m.id, m.org_id, m.display_name, m.public_email
            FROM mandate m
           WHERE {FILTER_SQL}
             AND m.public_email IS NOT NULL
             AND position('@' in m.public_email) > 1
             AND m.public_email NOT ILIKE '%@parlamento.democracia.social.br'
             AND NOT EXISTS (SELECT 1 FROM mandate_identity_binding b
                              WHERE b.mandate_id = m.id)
             AND NOT EXISTS (SELECT 1 FROM mandate_invite i
                              WHERE i.mandate_id = m.id
                                AND ((i.accepted_at IS NOT NULL)
                                  OR (i.accepted_at IS NULL AND i.revoked_at IS NULL
                                      AND i.expires_at > now())))
           ORDER BY m.uf NULLS LAST, m.display_name
           LIMIT $5",
    );
    let eligible: Vec<(Uuid, Uuid, String, String)> = match sqlx::query_as(&sql)
        .bind(sphere_or_default(&body.filter))
        .bind(&body.filter.house)
        .bind(&body.filter.uf)
        .bind(&body.filter.party)
        .bind(limit)
        .fetch_all(&state.db)
        .await
    {
        Ok(rows) => rows,
        Err(err) => return storage_err(err),
    };

    let svc = dsoc_auth::http::mandate_invite_service(&state);
    let mut items = Vec::with_capacity(eligible.len());
    for (mandate_id, org_id, display_name, email) in eligible {
        let outcome = svc
            .send(
                OrgId::from_uuid(org_id),
                CitizenId::from_uuid(admin),
                mandate_id,
                &email,
            )
            .await;
        match outcome {
            Ok(_) => items.push(BatchItemDto {
                mandate_id,
                display_name,
                email,
                ok: true,
                error: None,
            }),
            Err(err) => {
                tracing::warn!(%mandate_id, ?err, "invite-campaign: envio falhou");
                items.push(BatchItemDto {
                    mandate_id,
                    display_name,
                    email,
                    ok: false,
                    error: Some(format!("{err}")),
                });
            }
        }
    }
    let sent = items.iter().filter(|i| i.ok).count();
    let failed = items.len() - sent;
    (
        StatusCode::OK,
        Json(ApiResponse::ok(BatchResultDto {
            attempted: items.len(),
            sent,
            failed,
            items,
        })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    /// pending | accepted | expired (default pending)
    status: Option<String>,
    limit: Option<i64>,
}

#[derive(Debug, Serialize)]
struct InviteRowDto {
    invite_id: Uuid,
    mandate_id: Uuid,
    display_name: String,
    office: String,
    party: Option<String>,
    uf: Option<String>,
    email: String,
    sent_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    accepted_at: Option<DateTime<Utc>>,
}

/// `GET /admin/invite-campaign/invites?status=pending|accepted|expired`.
async fn list_invites(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Response {
    if let Err(resp) = require_admin(&headers, &state.db).await {
        return resp;
    }
    let status = q.status.as_deref().unwrap_or("pending");
    let cond = match status {
        "pending" => "i.accepted_at IS NULL AND i.revoked_at IS NULL AND i.expires_at > now()",
        "accepted" => "i.accepted_at IS NOT NULL",
        "expired" => "i.accepted_at IS NULL AND i.revoked_at IS NULL AND i.expires_at <= now()",
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::<()>::fail(
                    "bad_request",
                    "status deve ser pending, accepted ou expired",
                )),
            )
                .into_response();
        }
    };
    let limit = q.limit.unwrap_or(200).clamp(1, 1000);
    let sql = format!(
        r"SELECT i.id, i.mandate_id, m.display_name, m.office, m.party, m.uf,
                 i.email, i.created_at, i.expires_at, i.accepted_at
            FROM mandate_invite i
            JOIN mandate m ON m.id = i.mandate_id
           WHERE {cond}
           ORDER BY i.created_at DESC
           LIMIT $1",
    );
    type Row = (
        Uuid,
        Uuid,
        String,
        String,
        Option<String>,
        Option<String>,
        String,
        DateTime<Utc>,
        DateTime<Utc>,
        Option<DateTime<Utc>>,
    );
    match sqlx::query_as::<_, Row>(&sql)
        .bind(limit)
        .fetch_all(&state.db)
        .await
    {
        Ok(rows) => {
            let dtos: Vec<InviteRowDto> = rows
                .into_iter()
                .map(
                    |(
                        invite_id,
                        mandate_id,
                        display_name,
                        office,
                        party,
                        uf,
                        email,
                        sent_at,
                        expires_at,
                        accepted_at,
                    )| InviteRowDto {
                        invite_id,
                        mandate_id,
                        display_name,
                        office,
                        party,
                        uf,
                        email,
                        sent_at,
                        expires_at,
                        accepted_at,
                    },
                )
                .collect();
            (StatusCode::OK, Json(ApiResponse::ok(dtos))).into_response()
        }
        Err(err) => storage_err(err),
    }
}

fn storage_err(err: impl std::fmt::Debug) -> Response {
    tracing::error!(?err, "invite_campaign: storage error");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiResponse::<()>::fail("storage_error", "Erro interno.")),
    )
        .into_response()
}

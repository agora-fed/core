//! Admin side of contributions from the fediverse: the approval queue and the blocks.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::authz_ext::require_org_admin;
use crate::forum_federation::path_to_handle;
use crate::forum_inbound::{is_canonical_actor, relay_item};

/// Router for the queue and block endpoints.
pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/admin/forums/inbound/queue", get(queue))
        .route(
            "/admin/forums/inbound/queue/{kind}/{id}/{decision}",
            post(decide),
        )
        .route(
            "/admin/forums/inbound/blocks",
            get(list_blocks).post(add_block),
        )
        .route(
            "/admin/forums/inbound/blocks/{kind}/{id}",
            delete(remove_block),
        )
        .with_state(state)
}

fn fail(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(ApiResponse::<()>::fail(code, message))).into_response()
}

fn storage_failure(err: &sqlx::Error) -> Response {
    tracing::warn!(?err, "forum inbound admin: storage failure");
    fail(
        StatusCode::INTERNAL_SERVER_ERROR,
        "storage_error",
        "Erro interno.",
    )
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct QueueItem {
    kind: String,
    id: Uuid,
    forum_path: String,
    actor_url: String,
    handle: Option<String>,
    title: Option<String>,
    body: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

async fn queue(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let admin = match require_org_admin(&state.db, &headers).await {
        Ok(admin) => admin,
        Err(denied) => return denied,
    };
    // Each server's oldest come first, so a flood from one cannot push the others past the limit.
    let rows: Result<Vec<QueueItem>, _> = sqlx::query_as(
        "SELECT kind, id, forum_path, actor_url, handle, title, body, created_at
           FROM (SELECT queued.*,
                        row_number() OVER (PARTITION BY remote_host ORDER BY created_at) AS turn
                   FROM (SELECT 'topic' AS kind, t.id, f.full_path AS forum_path,
                                t.remote_actor_url AS actor_url, t.remote_handle AS handle,
                                t.remote_host, t.title, t.body, t.created_at
                           FROM forum_topic t JOIN forum f ON f.id = t.forum_id
                          WHERE t.moderation = 'pending' AND t.deleted_by IS NULL AND f.org_id = $1
                         UNION ALL
                         SELECT 'comment', c.id, f.full_path, c.remote_actor_url, c.remote_handle,
                                c.remote_host, t.title, c.body, c.created_at
                           FROM forum_topic_comment c
                           JOIN forum_topic t ON t.id = c.topic_id
                           JOIN forum f ON f.id = t.forum_id
                          WHERE c.moderation = 'pending' AND c.deleted_by IS NULL
                            AND f.org_id = $1) queued) ranked
          ORDER BY turn, created_at LIMIT 200",
    )
    .bind(admin.org)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => Json(ApiResponse::ok(rows)).into_response(),
        Err(err) => storage_failure(&err),
    }
}

async fn decide(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((kind, id, decision)): Path<(String, Uuid, String)>,
) -> Response {
    let admin = match require_org_admin(&state.db, &headers).await {
        Ok(admin) => admin,
        Err(denied) => return denied,
    };
    let approve = match decision.as_str() {
        "approve" => true,
        "reject" => false,
        _ => return fail(StatusCode::NOT_FOUND, "not_found", "Ação inválida."),
    };
    if !matches!(kind.as_str(), "topic" | "comment") {
        return fail(StatusCode::NOT_FOUND, "not_found", "Tipo inválido.");
    }
    let host = crate::forum_inbound::public_host();
    match decide_item(&state.db, host.as_deref(), admin.org, &kind, id, approve).await {
        Ok(true) => Json(ApiResponse::ok(serde_json::json!({ "decided": true }))).into_response(),
        Ok(false) => fail(StatusCode::NOT_FOUND, "not_found", "Item não está na fila."),
        Err(err) => storage_failure(&err),
    }
}

/// Approve or reject one queued item. `host` names the Group when repeating an approval; without
/// it the decision still applies and nothing is announced.
///
/// # Errors
/// Propagates storage failures.
pub async fn decide_item(
    db: &PgPool,
    host: Option<&str>,
    org: Uuid,
    kind: &str,
    id: Uuid,
    approve: bool,
) -> Result<bool, sqlx::Error> {
    if kind == "topic" {
        decide_topic(db, host, org, id, approve).await
    } else {
        decide_comment(db, host, org, id, approve).await
    }
}

async fn decide_topic(
    db: &PgPool,
    host: Option<&str>,
    org: Uuid,
    id: Uuid,
    approve: bool,
) -> Result<bool, sqlx::Error> {
    let row: Option<(String, bool)> = sqlx::query_as(
        "UPDATE forum_topic t
            SET moderation = $3,
                hidden_at = CASE WHEN $4 THEN NULL ELSE t.hidden_at END
           FROM forum f
          WHERE f.id = t.forum_id AND t.id = $1 AND f.org_id = $2
            AND t.moderation = 'pending' AND t.deleted_by IS NULL
         RETURNING f.full_path, t.relayed",
    )
    .bind(id)
    .bind(org)
    .bind(if approve { "approved" } else { "rejected" })
    .bind(approve)
    .fetch_optional(db)
    .await?;
    let Some((path, relayed)) = row else {
        return Ok(false);
    };
    after_decision(db, host, "topic", id, &path, approve, relayed).await?;
    Ok(true)
}

async fn decide_comment(
    db: &PgPool,
    host: Option<&str>,
    org: Uuid,
    id: Uuid,
    approve: bool,
) -> Result<bool, sqlx::Error> {
    let row: Option<(Uuid, String, bool)> = sqlx::query_as(
        "UPDATE forum_topic_comment c
            SET moderation = $3
           FROM forum_topic t, forum f
          WHERE t.id = c.topic_id AND f.id = t.forum_id AND c.id = $1 AND f.org_id = $2
            AND c.moderation = 'pending' AND c.deleted_by IS NULL
         RETURNING c.topic_id, f.full_path, c.relayed",
    )
    .bind(id)
    .bind(org)
    .bind(if approve { "approved" } else { "rejected" })
    .fetch_optional(db)
    .await?;
    let Some((topic, path, relayed)) = row else {
        return Ok(false);
    };
    dsoc_forums::queries::refresh_topic_counters(db, topic).await?;
    after_decision(db, host, "comment", id, &path, approve, relayed).await?;
    Ok(true)
}

/// Tell followers. Rejecting an edit of something they already have retracts it, since what they
/// hold is the version before the edit and the author no longer stands behind it.
async fn after_decision(
    db: &PgPool,
    host: Option<&str>,
    kind: &str,
    id: Uuid,
    path: &str,
    approve: bool,
    relayed: bool,
) -> Result<(), sqlx::Error> {
    let Some(host) = host else {
        return Ok(());
    };
    if approve {
        relay_item(db, host, &path_to_handle(path), kind, id).await
    } else if relayed {
        crate::forum_outbound::enqueue_removal(db, host, kind, id, None).await
    } else {
        Ok(())
    }
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct BlockRow {
    kind: String,
    id: Uuid,
    value: String,
    reason: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
}

async fn list_blocks(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(denied) = require_org_admin(&state.db, &headers).await {
        return denied;
    }
    let rows: Result<Vec<BlockRow>, _> = sqlx::query_as(
        "SELECT 'actor' AS kind, id, actor_url AS value, reason, created_at
           FROM forum_remote_actor_block
         UNION ALL
         SELECT 'domain', id, domain, reason, created_at
           FROM server_domain_block WHERE severity = 'suspend'
         ORDER BY created_at DESC LIMIT 500",
    )
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => Json(ApiResponse::ok(rows)).into_response(),
        Err(err) => storage_failure(&err),
    }
}

#[derive(Debug, Deserialize)]
struct NewBlock {
    kind: String,
    value: String,
    reason: Option<String>,
}

/// Block an account (its actor URL) or a server (its host), and drop what they left in the queue.
async fn add_block(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(block): Json<NewBlock>,
) -> Response {
    let admin = match require_org_admin(&state.db, &headers).await {
        Ok(admin) => admin,
        Err(denied) => return denied,
    };
    let value = block.value.trim().to_owned();
    let reason = block
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty());
    let result = match block.kind.as_str() {
        "actor" if is_canonical_actor(&value) => {
            sqlx::query(
                "INSERT INTO forum_remote_actor_block (id, actor_url, reason, created_by)
                 VALUES ($1, $2, $3, $4) ON CONFLICT (actor_url) DO NOTHING",
            )
            .bind(Uuid::now_v7())
            .bind(&value)
            .bind(reason)
            .bind(admin.citizen)
            .execute(&state.db)
            .await
        }
        "domain" if valid_domain(&value) => {
            sqlx::query(
                "INSERT INTO server_domain_block (id, domain, severity, reason, created_by)
                 VALUES ($1, $2, 'suspend', $3, $4)
                 ON CONFLICT (domain) DO UPDATE SET severity = 'suspend'",
            )
            .bind(Uuid::now_v7())
            .bind(value.to_ascii_lowercase())
            .bind(reason)
            .bind(admin.citizen)
            .execute(&state.db)
            .await
        }
        _ => {
            return fail(
                StatusCode::BAD_REQUEST,
                "validation",
                "Informe a URL https exata da conta ou o domínio do servidor.",
            )
        }
    };
    if let Err(err) = result {
        return storage_failure(&err);
    }
    if let Err(err) = reject_queued_from(&state.db, &block.kind, &value).await {
        return storage_failure(&err);
    }
    (
        StatusCode::CREATED,
        Json(ApiResponse::ok(serde_json::json!({ "blocked": true }))),
    )
        .into_response()
}

fn valid_domain(value: &str) -> bool {
    value.len() <= 253
        && value.contains('.')
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

async fn reject_queued_from(db: &PgPool, kind: &str, value: &str) -> Result<(), sqlx::Error> {
    let host = value.to_ascii_lowercase();
    for table in ["forum_topic", "forum_topic_comment"] {
        // `table` only takes the two constants above.
        let sql = format!(
            "UPDATE {table} SET moderation = 'rejected'
              WHERE moderation = 'pending'
                AND (($1 = 'actor' AND remote_actor_url = $2)
                  OR ($1 = 'domain' AND substring(remote_actor_url from 'https://([^/:]+)') = $3))"
        );
        sqlx::query(&sql)
            .bind(kind)
            .bind(value)
            .bind(&host)
            .execute(db)
            .await?;
    }
    Ok(())
}

async fn remove_block(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((kind, id)): Path<(String, Uuid)>,
) -> Response {
    if let Err(denied) = require_org_admin(&state.db, &headers).await {
        return denied;
    }
    let sql = match kind.as_str() {
        "actor" => "DELETE FROM forum_remote_actor_block WHERE id = $1",
        "domain" => "DELETE FROM server_domain_block WHERE id = $1",
        _ => return fail(StatusCode::NOT_FOUND, "not_found", "Tipo inválido."),
    };
    match sqlx::query(sql).bind(id).execute(&state.db).await {
        Ok(_) => Json(ApiResponse::ok(serde_json::json!({ "removed": true }))).into_response(),
        Err(err) => storage_failure(&err),
    }
}

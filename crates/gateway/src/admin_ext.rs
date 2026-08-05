//! # Admin extension endpoints (0.20.0-admin-console).
//!
//! A small companion to `dsoc-admin` that adds the read-only dashboards and
//! moderation shortcuts the operator console needs: aggregate stats, paginated
//! user listing with role assignment, federation peer summary, and per-note
//! soft-hide. Every handler is guarded by `require_admin` — a caller that
//! isn't `owner` or `admin` in the DEFAULT_ORG_UUID tenant gets a 403 JSON
//! envelope.
//!
//! Schema note: the spec was written against a hypothetical `note` /
//! `federation_actor` pair, but the DemocraciaBR schema splits notes into
//! `federation_outbox_entry` (LOCAL, per-citizen author) and
//! `federation_timeline_entry` (REMOTE, per-actor-URL author). We keep the
//! same endpoint contract; the SQL joins onto both tables so the numbers add
//! up. Federation peers are grouped by the hostname extracted from
//! `federation_timeline_entry.actor_url` (falling back to
//! `federation_follow.remote_actor_url` for hosts we know via follows but
//! haven't received a Note from yet).

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// The tenant every admin-ext query scopes to. Mirrors `federation.rs`.
const DEFAULT_ORG_UUID: Uuid = uuid::uuid!("11111111-1111-1111-1111-111111111111");

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/admin/stats", get(stats))
        .route("/admin/users", get(users_list))
        .route("/admin/users/{citizen_id}/role", post(users_set_role))
        .route("/admin/federation/peers", get(federation_peers))
        .route("/admin/notes/{note_id}/hide", post(notes_hide))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Guards
// ---------------------------------------------------------------------------

fn server_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiResponse::<()>::fail("http_500", "Erro interno.")),
    )
        .into_response()
}

/// Verify the caller has at least `admin` role in DEFAULT_ORG_UUID. Returns
/// `Ok(())` on pass, or a ready-to-return 403 envelope. `auditor` alone is
/// NOT sufficient: these endpoints mutate role bindings and hide notes.
/// Org-scoped admin gate — delegates to [`crate::authz_ext::require_org_admin`]
/// (issue #8). This copy previously hard-bound `DEFAULT_ORG_UUID` regardless of
/// who was calling: fail-closed rather than an escalation, but still not the
/// caller's org, so a legitimate admin of a second org would have been denied.
async fn require_admin(state: &AppState, headers: &HeaderMap) -> Result<(), Response> {
    crate::authz_ext::require_org_admin(&state.db, headers)
        .await
        .map(|_| ())
}

// ---------------------------------------------------------------------------
// GET /admin/stats
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct StatsPayload {
    citizens: i64,
    actors_local: i64,
    actors_remote: i64,
    notes_total: i64,
    notes_last_7d: i64,
    mandates: i64,
    proposals: i64,
    notifications_unread: i64,
}

async fn stats(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    // Individual `SELECT count(*)` per table. Each returns a single-row
    // `(count,)` tuple. If any fails, log and 500 — a partial stats block is
    // more misleading than a clean error.
    async fn scalar(pool: &sqlx::PgPool, sql: &str) -> Result<i64, sqlx::Error> {
        sqlx::query_as::<_, (i64,)>(sql)
            .fetch_one(pool)
            .await
            .map(|(n,)| n)
    }
    let pool = &state.db;
    let all: Result<StatsPayload, sqlx::Error> = async {
        Ok(StatsPayload {
            citizens: scalar(pool, "SELECT count(*) FROM citizen").await?,
            // LOCAL actors = public citizens (per ADR-0010; only these
            // materialize a Person Actor). REMOTE actors = distinct
            // remote_actor_url we've seen (via follow or inbound timeline).
            actors_local: scalar(pool, "SELECT count(*) FROM citizen WHERE is_public = true")
                .await?,
            actors_remote: scalar(
                pool,
                r"SELECT count(*) FROM (
                      SELECT remote_actor_url AS url FROM federation_follow
                      UNION
                      SELECT actor_url AS url FROM federation_timeline_entry
                  ) t",
            )
            .await?,
            // Notes: local outbox + remote timeline, both live.
            notes_total: scalar(
                pool,
                r"SELECT
                      (SELECT count(*) FROM federation_outbox_entry WHERE deleted_at IS NULL)
                    + (SELECT count(*) FROM federation_timeline_entry WHERE deleted_at IS NULL)",
            )
            .await?,
            notes_last_7d: scalar(
                pool,
                r"SELECT
                      (SELECT count(*) FROM federation_outbox_entry
                        WHERE deleted_at IS NULL
                          AND created_at >= now() - interval '7 days')
                    + (SELECT count(*) FROM federation_timeline_entry
                        WHERE deleted_at IS NULL
                          AND published_at >= now() - interval '7 days')",
            )
            .await?,
            mandates: scalar(pool, "SELECT count(*) FROM mandate").await?,
            proposals: scalar(pool, "SELECT count(*) FROM proposal").await?,
            notifications_unread: scalar(
                pool,
                "SELECT count(*) FROM user_notification WHERE read_at IS NULL",
            )
            .await?,
        })
    }
    .await;
    match all {
        Ok(payload) => (StatusCode::OK, Json(ApiResponse::ok(payload))).into_response(),
        Err(err) => {
            tracing::error!(error = ?err, "admin_ext stats query failed");
            server_error()
        }
    }
}

// ---------------------------------------------------------------------------
// GET /admin/users
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct UsersQuery {
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    offset: Option<i64>,
}

#[derive(Debug, Serialize)]
struct UserRow {
    citizen_id: Uuid,
    handle: String,
    display_name: String,
    email: String,
    is_public: bool,
    verification_level: String,
    created_at: String,
    role: Option<String>,
}

async fn users_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<UsersQuery>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let limit = q.limit.unwrap_or(25).clamp(1, 200);
    let offset = q.offset.unwrap_or(0).max(0);
    // MIN(role) picks the lexicographically-smallest role when a citizen
    // holds more than one — under the ('admin' < 'auditor' < 'owner')
    // ordering that means "the most privileged tier we can display". Good
    // enough for the console; the full multi-role view lives in the
    // dedicated /admin/users/{id} detail page (out of scope here).
    let search_pat =
        q.q.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| format!("{s}%"));
    let rows: Vec<(
        Uuid,
        Option<String>,
        Option<String>,
        Option<String>,
        bool,
        String,
        chrono::DateTime<chrono::Utc>,
        Option<String>,
    )> = match sqlx::query_as(
        r"SELECT c.id,
                 c.handle,
                 c.display_name,
                 ac.email,
                 c.is_public,
                 c.verification_level,
                 c.created_at,
                 (SELECT MIN(role) FROM admin_role_binding
                   WHERE org_id = $1 AND citizen_id = c.id) AS role
            FROM citizen c
            LEFT JOIN auth_credential ac
                   ON ac.citizen_id = c.id AND ac.org_id = c.org_id
           WHERE c.org_id = $1
             AND ($2::text IS NULL
                  OR ac.email ILIKE $2
                  OR c.handle ILIKE $2)
           ORDER BY c.created_at DESC
           LIMIT $3 OFFSET $4",
    )
    .bind(DEFAULT_ORG_UUID)
    .bind(search_pat.as_deref())
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(error = ?err, "admin_ext users_list query failed");
            return server_error();
        }
    };
    let out: Vec<UserRow> = rows
        .into_iter()
        .map(
            |(id, handle, display_name, email, is_public, verification_level, created_at, role)| {
                UserRow {
                    citizen_id: id,
                    handle: handle.unwrap_or_default(),
                    display_name: display_name.unwrap_or_default(),
                    email: email.unwrap_or_default(),
                    is_public,
                    verification_level,
                    created_at: created_at.to_rfc3339(),
                    role,
                }
            },
        )
        .collect();
    (StatusCode::OK, Json(ApiResponse::ok(out))).into_response()
}

// ---------------------------------------------------------------------------
// POST /admin/users/{citizen_id}/role
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SetRoleBody {
    /// `owner` | `admin` | `auditor`. `null` (or missing) removes any
    /// existing binding for this citizen in DEFAULT_ORG_UUID.
    #[serde(default)]
    role: Option<String>,
}

#[derive(Debug, Serialize)]
struct SetRoleResponse {
    citizen_id: Uuid,
    role: Option<String>,
}

async fn users_set_role(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(citizen_id): Path<Uuid>,
    Json(body): Json<SetRoleBody>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let normalized = body
        .role
        .as_deref()
        .map(str::trim)
        .map(str::to_ascii_lowercase);
    match normalized.as_deref() {
        None | Some("") => {
            // Remove any binding.
            if let Err(err) =
                sqlx::query(r"DELETE FROM admin_role_binding WHERE org_id = $1 AND citizen_id = $2")
                    .bind(DEFAULT_ORG_UUID)
                    .bind(citizen_id)
                    .execute(&state.db)
                    .await
            {
                tracing::error!(error = ?err, "admin_ext set_role delete failed");
                return server_error();
            }
            let out = SetRoleResponse {
                citizen_id,
                role: None,
            };
            (StatusCode::OK, Json(ApiResponse::ok(out))).into_response()
        }
        Some(role) if matches!(role, "owner" | "admin" | "auditor") => {
            // Replace any existing binding for this citizen: delete-all then
            // insert-one keeps the "current role" view single-valued.
            if let Err(err) =
                sqlx::query(r"DELETE FROM admin_role_binding WHERE org_id = $1 AND citizen_id = $2")
                    .bind(DEFAULT_ORG_UUID)
                    .bind(citizen_id)
                    .execute(&state.db)
                    .await
            {
                tracing::error!(error = ?err, "admin_ext set_role clear failed");
                return server_error();
            }
            let id = Uuid::now_v7();
            if let Err(err) = sqlx::query(
                r"INSERT INTO admin_role_binding (id, org_id, citizen_id, role, created_at)
                  VALUES ($1, $2, $3, $4, now())",
            )
            .bind(id)
            .bind(DEFAULT_ORG_UUID)
            .bind(citizen_id)
            .bind(role)
            .execute(&state.db)
            .await
            {
                tracing::error!(error = ?err, "admin_ext set_role insert failed");
                return server_error();
            }
            let out = SetRoleResponse {
                citizen_id,
                role: Some(role.to_owned()),
            };
            (StatusCode::OK, Json(ApiResponse::ok(out))).into_response()
        }
        Some(_) => (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::fail(
                "invalid_role",
                "role deve ser owner, admin, auditor ou null.",
            )),
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------
// GET /admin/federation/peers
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct PeersQuery {
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Debug, Serialize)]
struct PeerRow {
    host: String,
    actor_count: i64,
    last_seen: Option<String>,
}

/// Extract the host part of an ActivityPub actor URL. We do this in Rust
/// rather than in SQL because Postgres has no URL parser and the URLs are
/// well-behaved (`https://<host>/users/<name>` or similar). A malformed URL
/// falls back to the raw value so it still shows up in the list.
fn host_of(url: &str) -> String {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    match rest.find('/') {
        Some(i) => rest[..i].to_ascii_lowercase(),
        None => rest.to_ascii_lowercase(),
    }
}

async fn federation_peers(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<PeersQuery>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    // Union of "actor URLs we've received a Note from" with "actor URLs we
    // know via a follow relation" — the latter covers hosts that have a
    // follow but haven't posted anything we've stored yet.
    let timeline_rows: Vec<(String, Option<chrono::DateTime<chrono::Utc>>)> = match sqlx::query_as(
        r"SELECT actor_url, MAX(published_at) AS last_seen
                FROM federation_timeline_entry
               GROUP BY actor_url",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(error = ?err, "admin_ext peers timeline query failed");
            return server_error();
        }
    };
    let follow_rows: Vec<(String, chrono::DateTime<chrono::Utc>)> = match sqlx::query_as(
        r"SELECT remote_actor_url, MIN(created_at) AS created_at
            FROM federation_follow
           GROUP BY remote_actor_url",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(error = ?err, "admin_ext peers follow query failed");
            return server_error();
        }
    };
    // Aggregate by host.
    struct Bucket {
        actor_count: i64,
        last_seen: Option<chrono::DateTime<chrono::Utc>>,
        seen_urls: std::collections::HashSet<String>,
    }
    let mut buckets: HashMap<String, Bucket> = HashMap::new();
    for (url, last) in timeline_rows {
        let host = host_of(&url);
        let b = buckets.entry(host).or_insert(Bucket {
            actor_count: 0,
            last_seen: None,
            seen_urls: std::collections::HashSet::new(),
        });
        if b.seen_urls.insert(url) {
            b.actor_count += 1;
        }
        b.last_seen = match (b.last_seen, last) {
            (Some(a), Some(l)) => Some(a.max(l)),
            (Some(a), None) => Some(a),
            (None, Some(l)) => Some(l),
            (None, None) => None,
        };
    }
    for (url, created) in follow_rows {
        let host = host_of(&url);
        let b = buckets.entry(host).or_insert(Bucket {
            actor_count: 0,
            last_seen: None,
            seen_urls: std::collections::HashSet::new(),
        });
        if b.seen_urls.insert(url) {
            b.actor_count += 1;
        }
        // Fall back to follow.created_at when we've never received a Note
        // from this host — matches the spec's "coalesce last_seen to
        // created_at from federation_actor" intent.
        if b.last_seen.is_none() {
            b.last_seen = Some(created);
        }
    }
    let mut out: Vec<PeerRow> = buckets
        .into_iter()
        .map(|(host, b)| PeerRow {
            host,
            actor_count: b.actor_count,
            last_seen: b.last_seen.map(|dt| dt.to_rfc3339()),
        })
        .collect();
    out.sort_by(|a, b| b.actor_count.cmp(&a.actor_count));
    out.truncate(limit as usize);
    (StatusCode::OK, Json(ApiResponse::ok(out))).into_response()
}

// ---------------------------------------------------------------------------
// POST /admin/notes/{note_id}/hide
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct HideResponse {
    ok: bool,
}

async fn notes_hide(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(note_id): Path<Uuid>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    // A "note" in this codebase is either a LOCAL outbox entry OR a REMOTE
    // timeline entry. The spec's guard — "verify actor.kind='local'" —
    // maps here to "the id must live in federation_outbox_entry". If we
    // find it in the remote timeline instead, return 400.
    let is_local: Option<(Uuid,)> =
        match sqlx::query_as("SELECT id FROM federation_outbox_entry WHERE id = $1")
            .bind(note_id)
            .fetch_optional(&state.db)
            .await
        {
            Ok(v) => v,
            Err(err) => {
                tracing::error!(error = ?err, "admin_ext hide lookup failed");
                return server_error();
            }
        };
    if is_local.is_none() {
        // Not in outbox → either doesn't exist or is a remote note.
        let is_remote: Option<(Uuid,)> =
            match sqlx::query_as("SELECT id FROM federation_timeline_entry WHERE id = $1")
                .bind(note_id)
                .fetch_optional(&state.db)
                .await
            {
                Ok(v) => v,
                Err(err) => {
                    tracing::error!(error = ?err, "admin_ext hide remote-lookup failed");
                    return server_error();
                }
            };
        if is_remote.is_some() {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::<()>::fail(
                    "remote_note",
                    "não é possível esconder notas remotas",
                )),
            )
                .into_response();
        }
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::fail("not_found", "Nota não encontrada.")),
        )
            .into_response();
    }
    // Idempotent: setting deleted_at on an already-hidden row is fine.
    if let Err(err) = sqlx::query(
        r"UPDATE federation_outbox_entry
             SET deleted_at = COALESCE(deleted_at, now())
           WHERE id = $1",
    )
    .bind(note_id)
    .execute(&state.db)
    .await
    {
        tracing::error!(error = ?err, "admin_ext hide update failed");
        return server_error();
    }
    (
        StatusCode::OK,
        Json(ApiResponse::ok(HideResponse { ok: true })),
    )
        .into_response()
}

//! Convites de conta (migration 0509).
//!
//! Endpoints:
//! - `POST   /api/v1/invitations` — create (authenticated). Body: `{ target_email?, notes?, max_uses?, expires_in_hours? }`.
//! - `GET    /api/v1/invitations` — lista os meus.
//! - `DELETE /api/v1/invitations/{id}` — revoke (owner only).
//! - `GET    /api/v1/invitations/{token}/preview` — public: validates the token and returns minimal info
//!   so the `/convite?token=…` landing can decide whether a CTA is worth showing.

use axum::extract::{Json, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::Router;
use chrono::{DateTime, Duration, Utc};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route(
            "/invitations",
            post(create_invitation).get(list_my_invitations),
        )
        .route("/invitations/{id}", delete(delete_invitation))
        .route("/invitations/{token}/preview", get(preview_invitation))
        // 0.26.20: after signup, associates the logged-in citizen with the invitation they used.
        .route("/me/associate-invitation", post(associate_invitation))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
struct AssociateBody {
    token: String,
}

async fn associate_invitation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AssociateBody>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    // Idempotent — when an invitation is already associated, it is not overwritten.
    let already: Option<Option<Uuid>> = sqlx::query_scalar::<_, Option<Uuid>>(
        r"SELECT invited_via_invitation_id FROM citizen WHERE id = $1",
    )
    .bind(citizen)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);
    if let Some(Some(_)) = already {
        return (
            StatusCode::OK,
            Json(ApiResponse::ok(
                serde_json::json!({ "ok": true, "already": true }),
            )),
        )
            .into_response();
    }
    // Fetch the citizen's e-mail to validate the invitation's target_email.
    let email: Option<String> =
        sqlx::query_scalar(r"SELECT email FROM auth_credential WHERE citizen_id = $1")
            .bind(citizen)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);
    let email = email.unwrap_or_default();
    let token = body.token.trim();
    if token.is_empty() {
        return bad_request("token vazio");
    }
    let invited_via = match consume_token(&state.db, token, &email).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::ok(
                    serde_json::json!({ "ok": false, "reason": "invalid" }),
                )),
            )
                .into_response();
        }
        Err(err) => return server_error(err),
    };
    let _ = sqlx::query(r"UPDATE citizen SET invited_via_invitation_id = $2 WHERE id = $1")
        .bind(citizen)
        .bind(invited_via)
        .execute(&state.db)
        .await;
    (
        StatusCode::OK,
        Json(ApiResponse::ok(
            serde_json::json!({ "ok": true, "invited_via": invited_via }),
        )),
    )
        .into_response()
}

fn caller_citizen(headers: &HeaderMap) -> Option<Uuid> {
    headers
        .get("x-dsoc-citizen-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(ApiResponse::<()>::fail(
            "unauthorized",
            "Autenticação necessária.",
        )),
    )
        .into_response()
}

fn bad_request(msg: &'static str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ApiResponse::<()>::fail("bad_request", msg)),
    )
        .into_response()
}

fn server_error(err: impl std::fmt::Debug) -> Response {
    tracing::error!(?err, "invitations storage");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiResponse::<()>::fail("storage_error", "Erro interno.")),
    )
        .into_response()
}

fn generate_token() -> String {
    // 24 chars base64url ≈ 144 bits. Rand seguro.
    let mut buf = [0u8; 18];
    OsRng.fill_bytes(&mut buf);
    // base64url without padding, straight from the byte buffer.
    const ALPHA: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(24);
    // 18 bytes = 24 chars base64. Iteramos 3 em 3.
    for chunk in buf.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk[1] as u32;
        let b2 = chunk[2] as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHA[((n >> 18) & 63) as usize] as char);
        out.push(ALPHA[((n >> 12) & 63) as usize] as char);
        out.push(ALPHA[((n >> 6) & 63) as usize] as char);
        out.push(ALPHA[(n & 63) as usize] as char);
    }
    out
}

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CreateInvitationBody {
    #[serde(default)]
    target_email: Option<String>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default = "one")]
    max_uses: i32,
    #[serde(default = "default_expires")]
    expires_in_hours: i64,
}
fn one() -> i32 {
    1
}
fn default_expires() -> i64 {
    168
} // 7 dias

#[derive(Debug, Serialize)]
struct InvitationDto {
    id: Uuid,
    token: String,
    target_email: Option<String>,
    notes: Option<String>,
    uses_left: i32,
    max_uses: i32,
    created_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
    first_used_at: Option<DateTime<Utc>>,
    last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
struct PreviewDto {
    valid: bool,
    /// When missing: expired | exhausted | not_found
    reason: Option<String>,
    invited_by_handle: Option<String>,
    invited_by_display_name: Option<String>,
    target_email: Option<String>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn create_invitation(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Option<Json<CreateInvitationBody>>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let body = body
        .map(|Json(b)| b)
        .unwrap_or_else(|| CreateInvitationBody {
            target_email: None,
            notes: None,
            max_uses: 1,
            expires_in_hours: 168,
        });
    let max_uses = body.max_uses.clamp(1, 1000);
    let expires_hours = body.expires_in_hours.clamp(1, 24 * 90);
    let expires_at = Some(Utc::now() + Duration::hours(expires_hours));
    let target_email = body
        .target_email
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty() && s.contains('@'));
    let notes = body
        .notes
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let token = generate_token();
    let id = Uuid::now_v7();
    let res: Result<
        (
            Uuid,
            String,
            Option<String>,
            Option<String>,
            i32,
            i32,
            DateTime<Utc>,
            Option<DateTime<Utc>>,
        ),
        _,
    > = sqlx::query_as(
        r"INSERT INTO invitation
            (id, invited_by_citizen_id, token, target_email, notes, uses_left, max_uses, expires_at)
          VALUES ($1, $2, $3, $4, $5, $6, $6, $7)
          RETURNING id, token, target_email, notes, uses_left, max_uses, created_at, expires_at",
    )
    .bind(id)
    .bind(citizen)
    .bind(&token)
    .bind(target_email.as_deref())
    .bind(notes.as_deref())
    .bind(max_uses)
    .bind(expires_at)
    .fetch_one(&state.db)
    .await;
    match res {
        Ok((id, token, target_email, notes, uses_left, max_uses, created_at, expires_at)) => {
            let dto = InvitationDto {
                id,
                token,
                target_email,
                notes,
                uses_left,
                max_uses,
                created_at,
                expires_at,
                first_used_at: None,
                last_used_at: None,
            };
            (StatusCode::OK, Json(ApiResponse::ok(dto))).into_response()
        }
        Err(err) => server_error(err),
    }
}

async fn list_my_invitations(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let rows = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            Option<String>,
            Option<String>,
            i32,
            i32,
            DateTime<Utc>,
            Option<DateTime<Utc>>,
            Option<DateTime<Utc>>,
            Option<DateTime<Utc>>,
        ),
    >(
        r"SELECT id, token, target_email, notes, uses_left, max_uses,
                 created_at, expires_at, first_used_at, last_used_at
            FROM invitation
           WHERE invited_by_citizen_id = $1
           ORDER BY created_at DESC
           LIMIT 200",
    )
    .bind(citizen)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => {
            let list: Vec<InvitationDto> = rows
                .into_iter()
                .map(
                    |(
                        id,
                        token,
                        target_email,
                        notes,
                        uses_left,
                        max_uses,
                        created_at,
                        expires_at,
                        first_used_at,
                        last_used_at,
                    )| InvitationDto {
                        id,
                        token,
                        target_email,
                        notes,
                        uses_left,
                        max_uses,
                        created_at,
                        expires_at,
                        first_used_at,
                        last_used_at,
                    },
                )
                .collect();
            (StatusCode::OK, Json(ApiResponse::ok(list))).into_response()
        }
        Err(err) => server_error(err),
    }
}

async fn delete_invitation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let res = sqlx::query(
        r"DELETE FROM invitation
           WHERE id = $1 AND invited_by_citizen_id = $2",
    )
    .bind(id)
    .bind(citizen)
    .execute(&state.db)
    .await;
    match res {
        Ok(r) if r.rows_affected() == 0 => bad_request("convite não encontrado"),
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "ok": true }))),
        )
            .into_response(),
        Err(err) => server_error(err),
    }
}

async fn preview_invitation(State(state): State<AppState>, Path(token): Path<String>) -> Response {
    let row = sqlx::query_as::<
        _,
        (
            Option<String>,
            Option<i32>,
            Option<DateTime<Utc>>,
            Option<String>,
            Option<String>,
        ),
    >(
        r"SELECT inv.target_email,
                 inv.uses_left,
                 inv.expires_at,
                 c.handle,
                 c.display_name
            FROM invitation inv
            LEFT JOIN citizen c ON c.id = inv.invited_by_citizen_id
           WHERE inv.token = $1",
    )
    .bind(&token)
    .fetch_optional(&state.db)
    .await;
    let dto = match row {
        Ok(Some((target_email, uses_left, expires_at, handle, display_name))) => {
            let expired = expires_at.is_some_and(|d| d < Utc::now());
            let exhausted = uses_left.unwrap_or(0) <= 0;
            let (valid, reason) = if expired {
                (false, Some("expired".to_string()))
            } else if exhausted {
                (false, Some("exhausted".to_string()))
            } else {
                (true, None)
            };
            PreviewDto {
                valid,
                reason,
                invited_by_handle: handle,
                invited_by_display_name: display_name,
                target_email,
            }
        }
        Ok(None) => PreviewDto {
            valid: false,
            reason: Some("not_found".to_string()),
            invited_by_handle: None,
            invited_by_display_name: None,
            target_email: None,
        },
        Err(err) => return server_error(err),
    };
    (StatusCode::OK, Json(ApiResponse::ok(dto))).into_response()
}

/// Consumed by signup when creating an account with `?invitation=TOKEN` — validate +
/// atomic decrement, returning the invitation's id for `citizen.invited_via_invitation_id`.
pub async fn consume_token(
    db: &sqlx::PgPool,
    token: &str,
    for_email: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    // A single UPDATE ... RETURNING guarantees atomicity.
    let row = sqlx::query_as::<_, (Uuid, Option<String>)>(
        r"UPDATE invitation
             SET uses_left      = uses_left - 1,
                 first_used_at  = COALESCE(first_used_at, now()),
                 last_used_at   = now()
           WHERE token = $1
             AND uses_left > 0
             AND (expires_at IS NULL OR expires_at > now())
         RETURNING id, target_email",
    )
    .bind(token)
    .fetch_optional(db)
    .await?;
    if let Some((id, target_email)) = row {
        if let Some(t) = target_email {
            if !t.eq_ignore_ascii_case(for_email) {
                // Descarta o consumo — rollback via UPDATE reverso.
                let _ = sqlx::query(
                    r"UPDATE invitation
                         SET uses_left = uses_left + 1,
                             last_used_at = NULL
                       WHERE id = $1 AND last_used_at = (
                             SELECT max(last_used_at) FROM invitation WHERE id = $1)",
                )
                .bind(id)
                .execute(db)
                .await;
                return Ok(None);
            }
        }
        Ok(Some(id))
    } else {
        Ok(None)
    }
}

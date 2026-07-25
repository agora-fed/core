//! # Grupos de campanha (0.39.0, migration 0527) — Fase 2.3.
//!
//! Canal proativo campanha→eleitor. Até aqui o político só REAGIA a demandas
//! com SLA; aqui ele CRIA um espaço, o eleitor entra, e a campanha publica
//! atualizações que os membros veem.
//!
//! - `POST   /me/campaign-group`         — cria/atualiza o grupo do político (gate: is_politico).
//! - `GET    /me/campaign-group`         — painel do dono: grupo + nº de membros + posts.
//! - `POST   /me/campaign-group/posts`   — o dono publica uma atualização.
//! - `GET    /campaign-groups/{id}`      — página PÚBLICA (nome, dono, nº membros, posts, sou_membro).
//! - `POST   /campaign-groups/{id}/join` — o eleitor entra (idempotente).
//! - `DELETE /campaign-groups/{id}/join` — o eleitor sai.

use std::collections::HashMap;

use axum::extract::{Json, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, Utc};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

const DEFAULT_ORG_UUID: Uuid = uuid::uuid!("11111111-1111-1111-1111-111111111111");
const MAX_NAME: usize = 80;
const MAX_DESCRICAO: usize = 500;
const MAX_POST: usize = 2000;
const MAX_QUESTION: usize = 300;
const POSTS_LIMIT: i64 = 100;
const POLLS_LIMIT: i64 = 50;
const ANSWERS: [&str; 3] = ["concordo", "neutro", "discordo"];

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/me/campaign-group", get(my_group).post(upsert_group))
        .route("/me/campaign-group/posts", post(add_post))
        .route("/me/campaign-group/polls", post(create_poll))
        .route("/me/campaign-group/polls/{poll_id}/close", post(close_poll))
        .route("/campaign-groups/{id}", get(public_view))
        .route("/campaign-groups/{id}/join", post(join).delete(leave))
        .route(
            "/campaign-groups/{id}/polls/{poll_id}/respond",
            post(respond_poll),
        )
        .with_state(state)
}

fn caller_citizen(headers: &HeaderMap) -> Option<Uuid> {
    headers
        .get("x-dsoc-citizen-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
}

fn fail(status: StatusCode, code: &str, msg: &str) -> Response {
    (status, Json(ApiResponse::<()>::fail(code, msg))).into_response()
}

fn unauthorized() -> Response {
    fail(
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "Autenticação necessária.",
    )
}

fn storage_error() -> Response {
    fail(
        StatusCode::INTERNAL_SERVER_ERROR,
        "storage_error",
        "Erro interno.",
    )
}

/// O mandato do político logado (o vínculo mais recente). `None` = não é político.
async fn caller_mandate(db: &PgPool, citizen: Uuid) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar::<_, Uuid>(
        r"SELECT mandate_id FROM mandate_identity_binding
           WHERE citizen_id = $1
           ORDER BY verified_at DESC
           LIMIT 1",
    )
    .bind(citizen)
    .fetch_optional(db)
    .await
}

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct UpsertGroupBody {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AddPostBody {
    pub body: String,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct PostDto {
    id: Uuid,
    body: String,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct GroupCore {
    id: Uuid,
    name: String,
    description: Option<String>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreatePollBody {
    pub question: String,
}

#[derive(Debug, Deserialize)]
pub struct RespondPollBody {
    pub answer: String,
}

#[derive(Debug, Serialize)]
struct PollTally {
    concordo: i64,
    neutro: i64,
    discordo: i64,
    total: i64,
}

#[derive(Debug, Serialize)]
struct PollDto {
    id: Uuid,
    question: String,
    status: String,
    created_at: DateTime<Utc>,
    tally: PollTally,
    /// Resposta do caller autenticado (`None` = não respondeu / anônimo).
    my_answer: Option<String>,
}

#[derive(Debug, Serialize)]
struct MyGroupDto {
    is_politico: bool,
    /// `None` = político sem grupo criado ainda.
    group: Option<GroupCore>,
    member_count: i64,
    posts: Vec<PostDto>,
    polls: Vec<PollDto>,
}

#[derive(Debug, Serialize)]
struct PublicGroupDto {
    id: Uuid,
    name: String,
    description: Option<String>,
    owner_display_name: Option<String>,
    owner_handle: Option<String>,
    mandate_id: Uuid,
    member_count: i64,
    /// `true` só quando o caller autenticado já é membro.
    sou_membro: bool,
    posts: Vec<PostDto>,
    polls: Vec<PollDto>,
}

// ---------------------------------------------------------------------------
// POST /me/campaign-group — cria/atualiza o grupo do político
// ---------------------------------------------------------------------------

async fn upsert_group(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<UpsertGroupBody>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let mandate = match caller_mandate(&state.db, citizen).await {
        Ok(Some(m)) => m,
        Ok(None) => {
            return fail(
                StatusCode::FORBIDDEN,
                "not_politico",
                "Grupos de campanha são exclusivos de contas vinculadas a mandato.",
            )
        }
        Err(err) => {
            tracing::error!(?err, "campaign_group upsert: mandate lookup");
            return storage_error();
        }
    };
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_name",
            "Nome de 1 a 80 caracteres.",
        );
    }
    let description = body
        .description
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    if let Some(d) = &description {
        if d.chars().count() > MAX_DESCRICAO {
            return fail(
                StatusCode::BAD_REQUEST,
                "invalid_description",
                "Descrição longa demais.",
            );
        }
    }

    // Upsert por mandato (UNIQUE mandate_id): cria na primeira vez, edita depois.
    let id: Result<Uuid, sqlx::Error> = sqlx::query_scalar(
        r"INSERT INTO campaign_group (org_id, mandate_id, owner_citizen_id, name, description)
          VALUES ($1, $2, $3, $4, $5)
          ON CONFLICT (mandate_id) DO UPDATE
            SET name = EXCLUDED.name, description = EXCLUDED.description
          RETURNING id",
    )
    .bind(DEFAULT_ORG_UUID)
    .bind(mandate)
    .bind(citizen)
    .bind(name)
    .bind(description.as_deref())
    .fetch_one(&state.db)
    .await;

    match id {
        Ok(id) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "id": id }))),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "campaign_group upsert: insert");
            storage_error()
        }
    }
}

// ---------------------------------------------------------------------------
// GET /me/campaign-group — painel do dono
// ---------------------------------------------------------------------------

async fn my_group(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let mandate = match caller_mandate(&state.db, citizen).await {
        Ok(Some(m)) => m,
        Ok(None) => {
            return (
                StatusCode::OK,
                Json(ApiResponse::ok(MyGroupDto {
                    is_politico: false,
                    group: None,
                    member_count: 0,
                    posts: Vec::new(),
                    polls: Vec::new(),
                })),
            )
                .into_response()
        }
        Err(err) => {
            tracing::error!(?err, "campaign_group my: mandate lookup");
            return storage_error();
        }
    };
    let group: Option<GroupCore> = match sqlx::query_as(
        r"SELECT id, name, description, created_at FROM campaign_group WHERE mandate_id = $1",
    )
    .bind(mandate)
    .fetch_optional(&state.db)
    .await
    {
        Ok(g) => g,
        Err(err) => {
            tracing::error!(?err, "campaign_group my: group");
            return storage_error();
        }
    };
    let (member_count, posts) = match &group {
        Some(g) => match load_count_and_posts(&state.db, g.id).await {
            Ok(v) => v,
            Err(err) => {
                tracing::error!(?err, "campaign_group my: count/posts");
                return storage_error();
            }
        },
        None => (0, Vec::new()),
    };
    let polls = match &group {
        Some(g) => match load_polls(&state.db, g.id, Some(citizen)).await {
            Ok(v) => v,
            Err(err) => {
                tracing::error!(?err, "campaign_group my: polls");
                return storage_error();
            }
        },
        None => Vec::new(),
    };
    (
        StatusCode::OK,
        Json(ApiResponse::ok(MyGroupDto {
            is_politico: true,
            group,
            member_count,
            posts,
            polls,
        })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// POST /me/campaign-group/posts — o dono publica
// ---------------------------------------------------------------------------

async fn add_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AddPostBody>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let text = body.body.trim();
    if text.is_empty() || text.chars().count() > MAX_POST {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_post",
            "Post de 1 a 2000 caracteres.",
        );
    }
    // Só o dono do grupo posta: acha o grupo pelo mandato do caller.
    let group_id: Option<Uuid> = match sqlx::query_scalar::<_, Uuid>(
        r"SELECT cg.id FROM campaign_group cg
           JOIN mandate_identity_binding mib ON mib.mandate_id = cg.mandate_id
          WHERE mib.citizen_id = $1
          LIMIT 1",
    )
    .bind(citizen)
    .fetch_optional(&state.db)
    .await
    {
        Ok(g) => g,
        Err(err) => {
            tracing::error!(?err, "campaign_group post: owner lookup");
            return storage_error();
        }
    };
    let Some(group_id) = group_id else {
        return fail(
            StatusCode::FORBIDDEN,
            "no_group",
            "Crie seu grupo de campanha antes de publicar.",
        );
    };
    let res: Result<Uuid, sqlx::Error> = sqlx::query_scalar(
        "INSERT INTO campaign_group_post (group_id, body) VALUES ($1, $2) RETURNING id",
    )
    .bind(group_id)
    .bind(text)
    .fetch_one(&state.db)
    .await;
    match res {
        Ok(id) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(serde_json::json!({ "id": id }))),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "campaign_group post: insert");
            storage_error()
        }
    }
}

// ---------------------------------------------------------------------------
// GET /campaign-groups/{id} — página pública
// ---------------------------------------------------------------------------

type PublicRow = (
    Uuid,
    String,
    Option<String>,
    Uuid,
    Option<String>,
    Option<String>,
);

async fn public_view(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let row: Option<PublicRow> = match sqlx::query_as(
        r"SELECT cg.id, cg.name, cg.description, cg.mandate_id, m.display_name, c.handle
            FROM campaign_group cg
            JOIN mandate m ON m.id = cg.mandate_id
            LEFT JOIN citizen c ON c.id = cg.owner_citizen_id
           WHERE cg.id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(r) => r,
        Err(err) => {
            tracing::error!(?err, "campaign_group public: fetch");
            return storage_error();
        }
    };
    let Some((id, name, description, mandate_id, owner_display_name, owner_handle)) = row else {
        return fail(StatusCode::NOT_FOUND, "not_found", "Grupo não encontrado.");
    };
    let (member_count, posts) = match load_count_and_posts(&state.db, id).await {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "campaign_group public: count/posts");
            return storage_error();
        }
    };
    // Sou membro? só quando há caller autenticado.
    let caller = caller_citizen(&headers);
    let sou_membro = if let Some(citizen) = caller {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM campaign_group_member WHERE group_id = $1 AND citizen_id = $2)",
        )
        .bind(id)
        .bind(citizen)
        .fetch_one(&state.db)
        .await
        .unwrap_or(false)
    } else {
        false
    };
    let polls = match load_polls(&state.db, id, caller).await {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "campaign_group public: polls");
            return storage_error();
        }
    };
    (
        StatusCode::OK,
        Json(ApiResponse::ok(PublicGroupDto {
            id,
            name,
            description,
            owner_display_name,
            owner_handle,
            mandate_id,
            member_count,
            sou_membro,
            posts,
            polls,
        })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// POST/DELETE /campaign-groups/{id}/join — entrar/sair
// ---------------------------------------------------------------------------

async fn join(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<Uuid>) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    // Idempotente: ON CONFLICT não faz nada. FK garante que o grupo existe.
    let res = sqlx::query(
        r"INSERT INTO campaign_group_member (group_id, citizen_id)
          VALUES ($1, $2)
          ON CONFLICT (group_id, citizen_id) DO NOTHING",
    )
    .bind(id)
    .bind(citizen)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "joined": true }))),
        )
            .into_response(),
        Err(sqlx::Error::Database(dberr)) if dberr.is_foreign_key_violation() => {
            fail(StatusCode::NOT_FOUND, "not_found", "Grupo não encontrado.")
        }
        Err(err) => {
            tracing::error!(?err, "campaign_group join");
            storage_error()
        }
    }
}

async fn leave(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let res =
        sqlx::query("DELETE FROM campaign_group_member WHERE group_id = $1 AND citizen_id = $2")
            .bind(id)
            .bind(citizen)
            .execute(&state.db)
            .await;
    match res {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "joined": false }))),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "campaign_group leave");
            storage_error()
        }
    }
}

// ---------------------------------------------------------------------------
// Shared
// ---------------------------------------------------------------------------

async fn load_count_and_posts(
    db: &PgPool,
    group_id: Uuid,
) -> Result<(i64, Vec<PostDto>), sqlx::Error> {
    let member_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM campaign_group_member WHERE group_id = $1")
            .bind(group_id)
            .fetch_one(db)
            .await?;
    let posts: Vec<PostDto> = sqlx::query_as(
        r"SELECT id, body, created_at FROM campaign_group_post
           WHERE group_id = $1
           ORDER BY created_at DESC, id DESC
           LIMIT $2",
    )
    .bind(group_id)
    .bind(POSTS_LIMIT)
    .fetch_all(db)
    .await?;
    Ok((member_count, posts))
}

// ---------------------------------------------------------------------------
// Enquetes dirigidas (0.45.0, migration 0532) — Fase 3.4
// ---------------------------------------------------------------------------

/// O grupo do político logado (via mandato). `None` = não tem grupo.
async fn owner_group(db: &PgPool, citizen: Uuid) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar::<_, Uuid>(
        r"SELECT cg.id FROM campaign_group cg
           JOIN mandate_identity_binding mib ON mib.mandate_id = cg.mandate_id
          WHERE mib.citizen_id = $1
          LIMIT 1",
    )
    .bind(citizen)
    .fetch_optional(db)
    .await
}

/// POST /me/campaign-group/polls — o dono abre uma enquete rápida.
async fn create_poll(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreatePollBody>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let question = body.question.trim();
    if question.is_empty() || question.chars().count() > MAX_QUESTION {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_question",
            "Pergunta de 1 a 300 caracteres.",
        );
    }
    let group_id = match owner_group(&state.db, citizen).await {
        Ok(Some(g)) => g,
        Ok(None) => {
            return fail(
                StatusCode::FORBIDDEN,
                "no_group",
                "Crie seu grupo de campanha antes de abrir enquetes.",
            )
        }
        Err(err) => {
            tracing::error!(?err, "campaign_group poll: owner");
            return storage_error();
        }
    };
    let res: Result<Uuid, sqlx::Error> = sqlx::query_scalar(
        "INSERT INTO campaign_group_poll (group_id, question) VALUES ($1, $2) RETURNING id",
    )
    .bind(group_id)
    .bind(question)
    .fetch_one(&state.db)
    .await;
    match res {
        Ok(id) => (
            StatusCode::CREATED,
            Json(ApiResponse::ok(serde_json::json!({ "id": id }))),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "campaign_group poll: insert");
            storage_error()
        }
    }
}

/// POST /me/campaign-group/polls/{poll_id}/close — o dono encerra.
async fn close_poll(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(poll_id): Path<Uuid>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let group_id = match owner_group(&state.db, citizen).await {
        Ok(Some(g)) => g,
        Ok(None) => {
            return fail(
                StatusCode::FORBIDDEN,
                "no_group",
                "Você não tem um grupo de campanha.",
            )
        }
        Err(err) => {
            tracing::error!(?err, "campaign_group poll close: owner");
            return storage_error();
        }
    };
    let res = sqlx::query(
        r"UPDATE campaign_group_poll SET status = 'closed', closed_at = now()
           WHERE id = $1 AND group_id = $2 AND status = 'open'",
    )
    .bind(poll_id)
    .bind(group_id)
    .execute(&state.db)
    .await;
    match res {
        Ok(r) if r.rows_affected() == 1 => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "status": "closed" }))),
        )
            .into_response(),
        Ok(_) => fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "Enquete aberta não encontrada.",
        ),
        Err(err) => {
            tracing::error!(?err, "campaign_group poll close");
            storage_error()
        }
    }
}

/// POST /campaign-groups/{id}/polls/{poll_id}/respond — o cidadão logado responde.
async fn respond_poll(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((group_id, poll_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<RespondPollBody>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    if !ANSWERS.contains(&body.answer.as_str()) {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_answer",
            "Resposta inválida.",
        );
    }
    // A enquete existe, pertence ao grupo e está aberta?
    let status: Option<String> = match sqlx::query_scalar(
        "SELECT status FROM campaign_group_poll WHERE id = $1 AND group_id = $2",
    )
    .bind(poll_id)
    .bind(group_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(s) => s,
        Err(err) => {
            tracing::error!(?err, "campaign_group poll respond: status");
            return storage_error();
        }
    };
    match status.as_deref() {
        None => {
            return fail(
                StatusCode::NOT_FOUND,
                "not_found",
                "Enquete não encontrada.",
            )
        }
        Some("open") => {}
        Some(_) => {
            return fail(
                StatusCode::CONFLICT,
                "closed",
                "Esta enquete está encerrada.",
            )
        }
    }
    let res = sqlx::query(
        r"INSERT INTO campaign_group_poll_response (poll_id, citizen_id, answer)
          VALUES ($1, $2, $3)
          ON CONFLICT (poll_id, citizen_id)
          DO UPDATE SET answer = EXCLUDED.answer, updated_at = now()",
    )
    .bind(poll_id)
    .bind(citizen)
    .bind(&body.answer)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "saved": true }))),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "campaign_group poll respond: upsert");
            storage_error()
        }
    }
}

/// Enquetes de um grupo com agregado por opção e (opcional) a resposta do caller.
async fn load_polls(
    db: &PgPool,
    group_id: Uuid,
    caller: Option<Uuid>,
) -> Result<Vec<PollDto>, sqlx::Error> {
    let rows: Vec<(Uuid, String, String, DateTime<Utc>)> = sqlx::query_as(
        r"SELECT id, question, status, created_at FROM campaign_group_poll
           WHERE group_id = $1 ORDER BY created_at DESC, id DESC LIMIT $2",
    )
    .bind(group_id)
    .bind(POLLS_LIMIT)
    .fetch_all(db)
    .await?;
    let ids: Vec<Uuid> = rows.iter().map(|r| r.0).collect();

    let tallies: Vec<(Uuid, String, i64)> = sqlx::query_as(
        r"SELECT poll_id, answer, count(*) FROM campaign_group_poll_response
           WHERE poll_id = ANY($1) GROUP BY poll_id, answer",
    )
    .bind(&ids)
    .fetch_all(db)
    .await?;
    let mut by_p: HashMap<Uuid, (i64, i64, i64)> = HashMap::new();
    for (pid, answer, n) in tallies {
        let e = by_p.entry(pid).or_insert((0, 0, 0));
        match answer.as_str() {
            "concordo" => e.0 += n,
            "neutro" => e.1 += n,
            "discordo" => e.2 += n,
            _ => {}
        }
    }

    let mut mine: HashMap<Uuid, String> = HashMap::new();
    if let Some(c) = caller {
        let rows2: Vec<(Uuid, String)> = sqlx::query_as(
            "SELECT poll_id, answer FROM campaign_group_poll_response
              WHERE citizen_id = $1 AND poll_id = ANY($2)",
        )
        .bind(c)
        .bind(&ids)
        .fetch_all(db)
        .await?;
        mine = rows2.into_iter().collect();
    }

    Ok(rows
        .into_iter()
        .map(|(id, question, status, created_at)| {
            let (concordo, neutro, discordo) = by_p.get(&id).copied().unwrap_or((0, 0, 0));
            PollDto {
                id,
                question,
                status,
                created_at,
                tally: PollTally {
                    concordo,
                    neutro,
                    discordo,
                    total: concordo + neutro + discordo,
                },
                my_answer: mine.get(&id).cloned(),
            }
        })
        .collect())
}

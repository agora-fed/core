//! Axum surface of the forums. Mutation requires an authenticated, verified
//! caller (Email level — the identity document is already validated at signup); reads are public.
//! [`ApiResponse`] envelope and error map identical to the other crates (ADR-0007).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use dsoc_api_contract::ApiResponse;
use dsoc_core::ids::OrgId;
use dsoc_core::{Error, VerificationLevel};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::domain::{NewTopic, Stance};
use crate::queries::{CommentRow, DispatchRow, ForumRow, TopicRow};
use crate::service::{ChildEntry, ForumService};

/// The instance's single organization (same convention as the single-org gateway).
const DEFAULT_ORG_UUID: Uuid = uuid::uuid!("11111111-1111-1111-1111-111111111111");

/// Public view of a forum.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ForumDto {
    /// Id.
    pub id: Uuid,
    /// Caminho completo (`sp/santos/saude`).
    pub full_path: String,
    /// Segmento.
    pub slug: String,
    /// Display name.
    pub name: String,
    /// Description.
    pub description: String,
    /// `institucional` | `governanca` | `comunitario`.
    pub kind: String,
    /// Sphere, when territorial.
    pub esfera: Option<String>,
    /// State, when territorial.
    pub uf: Option<String>,
    /// Municipality, when territorial.
    pub municipio: Option<String>,
    /// Whether an institutional e-mail is linked (the address itself is never exposed).
    pub has_contact_email: bool,
    /// Forum logo (0543).
    pub avatar_url: Option<String>,
    /// Forum banner (0543).
    pub banner_url: Option<String>,
    /// Configured dispatch thresholds.
    pub thresholds: Vec<i32>,
}

/// Transparency of the city council (`civic_source` catalog, 0662/0669).
/// Only applies to municipal-sphere forums; uses the ABSENCE of open data as a
/// public demand, and points at the council's official site when one exists.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TransparencyDto {
    /// `plena` (open data, offices connected) | `parcial` (has a site, no
    /// open data) | `ausente` (no portal found).
    pub status: String,
    /// Official council site, when known (`base_url`). `None` when `ausente`.
    pub official_url: Option<String>,
}

/// A child in the tree (materialized, or a virtual section from the template).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ForumChildDto {
    /// Segmento.
    pub slug: String,
    /// Caminho completo.
    pub full_path: String,
    /// Nome.
    pub name: String,
    /// Default section with no topics yet (materialized on first use).
    pub virtual_section: bool,
}

/// Tree response for one level.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ForumTreeDto {
    /// The requested forum (absent at the root).
    pub forum: Option<ForumDto>,
    /// Filhos (reais + virtuais).
    pub children: Vec<ForumChildDto>,
}

/// Public view of a topic.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TopicDto {
    /// Id.
    pub id: Uuid,
    /// Owning forum.
    pub forum_id: Uuid,
    /// Title.
    pub title: String,
    /// Corpo.
    pub body: String,
    /// Opaque public handle of the author (`u-<hex>`).
    pub author_public_handle: String,
    /// COUNTABLE interactions (votes + local comments) — these fire thresholds.
    pub interactions: i64,
    /// FEDERATED interactions (these never fire).
    pub federated_interactions: i64,
    /// Saldo favor - contra.
    pub score: i64,
    /// Stances in favour (debates→forums merge, 0544).
    pub favor: i64,
    /// Stances against.
    pub contra: i64,
    /// Neutral stances (legacy).
    pub ponderacao: i64,
    /// Approved comments.
    pub comment_count: i64,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// A comment (local or federated).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ForumCommentDto {
    /// Id.
    pub id: Uuid,
    /// Autor local (handle opaco) OU handle remoto.
    pub author: String,
    /// Karma (StackOverflow-style reputation) of the local author (ADR-0019). `None` = federated/no author.
    pub author_karma: Option<i32>,
    /// Veio do fediverso?
    pub federated: bool,
    /// Stance declared alongside the argument (`favor`|`contra`|`ponderacao`|null).
    pub stance: Option<String>,
    /// Votes in favour of this argument (0545).
    pub favor: i64,
    /// Votos contra.
    pub contra: i64,
    /// Neutral stances (legacy).
    pub ponderacao: i64,
    /// Corpo.
    pub body: String,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Public receipt of an institutional dispatch, per threshold.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DispatchDto {
    /// Patamar cruzado.
    pub threshold: i32,
    /// When the e-mail left the relay (None = still queued).
    pub sent_at: Option<DateTime<Utc>>,
    /// Momento do cruzamento.
    pub crossed_at: DateTime<Utc>,
}

/// Detail: topic + comments + receipts.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TopicDetailDto {
    /// The topic.
    pub topic: TopicDto,
    /// Approved comments (local + federated).
    pub comments: Vec<ForumCommentDto>,
    /// Institutional dispatch receipts.
    pub dispatches: Vec<DispatchDto>,
    /// This forum's effective PROPORTIONAL dispatch threshold (D3): the score
    /// the scoreboard must cross to summon the office, proportional to the
    /// territory's electorate (floor 10). The UI uses this instead of a fixed 10.
    pub escalation_threshold: i64,
    /// Graduated privacy (D5/D6): `true` when the forum belongs to a small
    /// municipality — in that case individual stance attribution was omitted from
    /// the comments (author pseudonymized, `stance`/karma null); only the topic
    /// aggregate (favor/contra/score) is public. The UI must signal this.
    pub aggregate_only: bool,
    /// WHOM the scoreboard dispatches to once the threshold is crossed: names of
    /// the reachable target mandates (B1, "Dep. Fulana · Sen. Beltrano") or the
    /// section name with a curated institutional contact (a ministry, say).
    /// Transportes"). `None` = no reachable channel yet — the UI shows a pending state.
    pub escalation_destination: Option<String>,
}

/// A **bridging claim** (D8.2): an argument endorsed ACROSS the topic's
/// for×against divide — what UNITES those who disagree, not the cheering scoreboard.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct BridgeCommentDto {
    /// The argument (same shape as comments; author pseudonymized in a small municipality).
    pub comment: ForumCommentDto,
    /// Endorsements from citizens who voted `favor` on the topic.
    pub favor_side: i64,
    /// Endorsements from citizens who voted `contra` on the topic.
    pub contra_side: i64,
    /// Bridge score = harmonic mean of both sides (higher = unites more).
    pub bridge_score: f64,
}

/// **Consensus** section of a topic (D8.2): the top bridging claims, ordered by
/// bridge score. An ADDITIVE layer on top of the for×against scoreboard.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TopicConsensusDto {
    /// Topic id.
    pub topic_id: Uuid,
    /// Bridging claims ordered by bridge score (desc); empty = no bridge yet.
    pub bridges: Vec<BridgeCommentDto>,
    /// Privacidade agregada ativa (D5/D6): autor pseudonimizado nos argumentos.
    pub aggregate_only: bool,
}

/// Topic creation.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateTopicRequest {
    /// Forum path (`sp/santos/saude`).
    pub path: String,
    /// Title.
    pub title: String,
    /// Corpo.
    pub body: String,
    /// OPTIONAL targets (B1): mandate_ids directing the demand at specific
    /// office(s). Absent/empty = topic with no target (dispatches to the
    /// section's curated contact). Duplicates are ignored; the total is capped in the service.
    #[serde(default)]
    pub targets: Vec<Uuid>,
}

/// The citizen's stance (debates→forums merge).
#[derive(Debug, Deserialize, ToSchema)]
pub struct VoteRequest {
    /// `favor` | `contra` | `ponderacao`.
    pub stance: Option<String>,
    /// Compat with older clients: +1 → favor, -1 → contra.
    pub value: Option<i16>,
}

impl VoteRequest {
    /// Resolve the requested stance (stance preferred; legacy `value` mapped).
    fn stance(&self) -> Result<Stance, Error> {
        if let Some(s) = &self.stance {
            return Stance::parse_input(s);
        }
        match self.value {
            Some(1) => Ok(Stance::Favor),
            Some(-1) => Ok(Stance::Contra),
            _ => Err(Error::Validation(
                "informe stance: favor, contra ou ponderacao".to_owned(),
            )),
        }
    }
}

/// Local comment (argument) — with an optional stance that also records the vote.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CommentRequest {
    /// Corpo.
    pub body: String,
    /// `favor` | `contra` | `ponderacao` (opcional).
    pub stance: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TreeParams {
    path: Option<String>,
    esfera: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TopicListParams {
    path: String,
    sort: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

fn forum_dto(r: ForumRow) -> ForumDto {
    ForumDto {
        id: r.id,
        full_path: r.full_path,
        slug: r.slug,
        name: r.name,
        description: r.description,
        kind: r.kind,
        esfera: r.esfera,
        uf: r.uf,
        municipio: r.municipio,
        has_contact_email: r.contact_email.is_some(),
        avatar_url: r.avatar_url,
        banner_url: r.banner_url,
        thresholds: r.thresholds,
    }
}

fn topic_dto(r: TopicRow) -> TopicDto {
    TopicDto {
        id: r.id,
        forum_id: r.forum_id,
        title: r.title,
        body: r.body,
        author_public_handle: format!("u-{}", r.author_id.as_simple()),
        interactions: r.interaction_count,
        federated_interactions: r.federated_interaction_count,
        score: r.score,
        favor: r.favor_count,
        contra: r.contra_count,
        ponderacao: r.ponderacao_count,
        comment_count: r.comment_count,
        created_at: r.created_at,
    }
}

/// `protect` (D5/D6): in a small municipality, individual stance attribution is
/// omitted — the author becomes a generic pseudonym and `stance`/karma vanish, so
/// no retaliation map (who supported/voted what) can be assembled. The argument's
/// BODY (public speech) and its aggregate counters stay visible; what is protected
/// is the person↔stance link, not the debate itself.
fn comment_dto(r: CommentRow, protect: bool) -> ForumCommentDto {
    let author = if protect {
        // A non-identifiable, per-topic-stable pseudonym is impossible without extra
        // state; we use a generic label (minimum viable).
        "participante".to_owned()
    } else {
        match (&r.author_id, &r.remote_handle) {
            (Some(cid), _) => format!("u-{}", cid.as_simple()),
            (None, Some(h)) => h.clone(),
            (None, None) => "(desconhecido)".to_owned(),
        }
    };
    ForumCommentDto {
        id: r.id,
        author,
        author_karma: if protect { None } else { r.author_karma },
        federated: r.federated,
        stance: if protect { None } else { r.stance },
        favor: r.favor_count,
        contra: r.contra_count,
        ponderacao: r.ponderacao_count,
        body: r.body,
        created_at: r.created_at,
    }
}

fn dispatch_dto(r: DispatchRow) -> DispatchDto {
    DispatchDto {
        threshold: r.threshold,
        sent_at: r.sent_at,
        crossed_at: r.created_at,
    }
}

fn child_dto(c: ChildEntry) -> ForumChildDto {
    ForumChildDto {
        slug: c.slug,
        full_path: c.full_path,
        name: c.name,
        virtual_section: c.virtual_section,
    }
}

/// The crate's router — mounted by the gateway under `/api/v1`.
pub fn routes(state: dsoc_app::AppState) -> Router<()> {
    Router::new()
        .route("/f/tree", get(tree))
        .route("/f/recent", get(recent))
        .route("/f/topics", post(create_topic).get(list_topics))
        .route("/f/topics/{id}", get(get_topic))
        .route("/f/topics/{id}/consensus", get(consensus))
        .route("/f/topics/{id}/vote", post(vote))
        .route("/f/topics/{id}/comments", post(comment))
        .route("/f/comments/{id}/vote", post(vote_comment))
        .with_state(state)
}

/// One item of the latest-posts feed (the /f home).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RecentTopicDto {
    /// Topic id.
    pub id: Uuid,
    /// Title.
    pub title: String,
    /// Saldo favor - contra.
    pub score: i64,
    /// Stances in favour.
    pub favor: i64,
    /// Stances against.
    pub contra: i64,
    /// Neutral stances (legacy).
    pub ponderacao: i64,
    /// Countable interactions.
    pub interactions: i64,
    /// Comments.
    pub comment_count: i64,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Forum path.
    pub forum_path: String,
    /// Forum name.
    pub forum_name: String,
}

/// `GET /f/recent?limit=` — latest posts across all forums (home feed).
async fn recent(
    State(state): State<dsoc_app::AppState>,
    Query(p): Query<RecentParams>,
) -> Response {
    let svc = ForumService::from_state(&state);
    let org = OrgId::from_uuid(DEFAULT_ORG_UUID);
    match svc.recent_topics(org, p.limit.unwrap_or(25)).await {
        Ok(rows) => {
            let dtos: Vec<RecentTopicDto> = rows
                .into_iter()
                .map(|r| RecentTopicDto {
                    id: r.id,
                    title: r.title,
                    score: r.score,
                    favor: r.favor_count,
                    contra: r.contra_count,
                    ponderacao: r.ponderacao_count,
                    interactions: r.interaction_count,
                    comment_count: r.comment_count,
                    created_at: r.created_at,
                    forum_path: r.forum_path,
                    forum_name: r.forum_name,
                })
                .collect();
            (StatusCode::OK, Json(ApiResponse::ok(dtos))).into_response()
        }
        Err(e) => error_response::<Vec<RecentTopicDto>>(&e),
    }
}

#[derive(Debug, Deserialize)]
struct RecentParams {
    limit: Option<i64>,
}

/// `GET /f/tree?path=&esfera=` — one level of the tree (root when `path` is absent).
async fn tree(State(state): State<dsoc_app::AppState>, Query(p): Query<TreeParams>) -> Response {
    let svc = ForumService::from_state(&state);
    let org = OrgId::from_uuid(DEFAULT_ORG_UUID);
    match svc.tree(org, p.path.as_deref(), p.esfera.as_deref()).await {
        Ok((forum, children)) => {
            let dto = ForumTreeDto {
                forum: forum.map(forum_dto),
                children: children.into_iter().map(child_dto).collect(),
            };
            (StatusCode::OK, Json(ApiResponse::ok(dto))).into_response()
        }
        Err(e) => error_response::<ForumTreeDto>(&e),
    }
}

/// `POST /f/topics` — create a topic (verified citizen; the identity document is validated at signup).
async fn create_topic(
    State(state): State<dsoc_app::AppState>,
    caller: dsoc_app::CallerId,
    Json(req): Json<CreateTopicRequest>,
) -> Response {
    if let Err(e) = state
        .authz
        .require(caller.org, caller.citizen, VerificationLevel::Email)
        .await
    {
        return error_response::<TopicDto>(&e);
    }
    let new = match NewTopic::validate(&req.title, &req.body) {
        Ok(n) => n,
        Err(e) => return error_response::<TopicDto>(&e),
    };
    let svc = ForumService::from_state(&state);
    match svc
        .create_topic(caller.org, &req.path, caller.citizen, &new, &req.targets)
        .await
    {
        Ok(row) => (StatusCode::CREATED, Json(ApiResponse::ok(topic_dto(row)))).into_response(),
        Err(e) => error_response::<TopicDto>(&e),
    }
}

/// `GET /f/topics?path=&sort=hot|new&limit=&offset=`.
async fn list_topics(
    State(state): State<dsoc_app::AppState>,
    Query(p): Query<TopicListParams>,
) -> Response {
    let svc = ForumService::from_state(&state);
    let org = OrgId::from_uuid(DEFAULT_ORG_UUID);
    let hot = p.sort.as_deref() != Some("new");
    match svc
        .list_topics(
            org,
            &p.path,
            hot,
            p.limit.unwrap_or(30),
            p.offset.unwrap_or(0),
        )
        .await
    {
        Ok((forum, topics)) => {
            // Transparency banner: municipal forums only, cross-referenced with
            // the civic_source catalog. ADDITIVE — degrades to `None` without breaking.
            let transparency =
                svc.municipal_transparency(&forum)
                    .await
                    .map(|(status, official_url)| TransparencyDto {
                        status,
                        official_url,
                    });
            let dto = serde_json::json!({
                "forum": forum_dto(forum),
                "topics": topics.into_iter().map(topic_dto).collect::<Vec<_>>(),
                "transparency": transparency,
            });
            (StatusCode::OK, Json(ApiResponse::ok(dto))).into_response()
        }
        Err(e) => error_response::<serde_json::Value>(&e),
    }
}

/// `GET /f/topics/{id}` — detail with comments and receipts.
async fn get_topic(State(state): State<dsoc_app::AppState>, Path(id): Path<Uuid>) -> Response {
    let svc = ForumService::from_state(&state);
    match svc.get_topic(id).await {
        Ok(d) => {
            let aggregate_only = d.aggregate_only;
            let dto = TopicDetailDto {
                topic: topic_dto(d.topic),
                comments: d
                    .comments
                    .into_iter()
                    .map(|c| comment_dto(c, aggregate_only))
                    .collect(),
                dispatches: d.dispatches.into_iter().map(dispatch_dto).collect(),
                escalation_threshold: d.escalation_threshold,
                aggregate_only,
                escalation_destination: d.escalation_destination,
            };
            (StatusCode::OK, Json(ApiResponse::ok(dto))).into_response()
        }
        Err(e) => error_response::<TopicDetailDto>(&e),
    }
}

#[derive(Debug, Deserialize)]
struct ConsensusParams {
    limit: Option<usize>,
}

/// `GET /f/topics/{id}/consensus?limit=` — the top bridging claims (D8.2):
/// arguments that gather support from BOTH sides of the topic, ordered by bridge
/// score. Public read (same policy as the other forum GETs). `limit`
/// defaults to 5, capped at 20 (enforced in the service).
async fn consensus(
    State(state): State<dsoc_app::AppState>,
    Path(id): Path<Uuid>,
    Query(p): Query<ConsensusParams>,
) -> Response {
    let svc = ForumService::from_state(&state);
    match svc.topic_consensus(id, p.limit.unwrap_or(5)).await {
        Ok(c) => {
            let protect = c.aggregate_only;
            let bridges: Vec<BridgeCommentDto> = c
                .bridges
                .into_iter()
                .map(|b| BridgeCommentDto {
                    comment: comment_dto(b.comment, protect),
                    favor_side: b.favor_side,
                    contra_side: b.contra_side,
                    bridge_score: b.bridge_score,
                })
                .collect();
            let dto = TopicConsensusDto {
                topic_id: id,
                bridges,
                aggregate_only: c.aggregate_only,
            };
            (StatusCode::OK, Json(ApiResponse::ok(dto))).into_response()
        }
        Err(e) => error_response::<TopicConsensusDto>(&e),
    }
}

/// `POST /f/topics/{id}/vote` — stance for/against/neutral (upsert;
/// LOCAL citizens only, by construction).
async fn vote(
    State(state): State<dsoc_app::AppState>,
    caller: dsoc_app::CallerId,
    Path(id): Path<Uuid>,
    Json(req): Json<VoteRequest>,
) -> Response {
    if let Err(e) = state
        .authz
        .require(caller.org, caller.citizen, VerificationLevel::Email)
        .await
    {
        return error_response::<TopicDto>(&e);
    }
    let stance = match req.stance() {
        Ok(s) => s,
        Err(e) => return error_response::<TopicDto>(&e),
    };
    let svc = ForumService::from_state(&state);
    match svc.vote(id, caller.citizen, stance).await {
        Ok(row) => (StatusCode::OK, Json(ApiResponse::ok(topic_dto(row)))).into_response(),
        Err(e) => error_response::<TopicDto>(&e),
    }
}

/// `POST /f/topics/{id}/comments` — local comment.
async fn comment(
    State(state): State<dsoc_app::AppState>,
    caller: dsoc_app::CallerId,
    Path(id): Path<Uuid>,
    Json(req): Json<CommentRequest>,
) -> Response {
    if let Err(e) = state
        .authz
        .require(caller.org, caller.citizen, VerificationLevel::Email)
        .await
    {
        return error_response::<TopicDto>(&e);
    }
    let stance = match req.stance.as_deref().map(Stance::parse_input).transpose() {
        Ok(s) => s,
        Err(e) => return error_response::<TopicDto>(&e),
    };
    let svc = ForumService::from_state(&state);
    match svc.comment(id, caller.citizen, &req.body, stance).await {
        Ok(row) => (StatusCode::CREATED, Json(ApiResponse::ok(topic_dto(row)))).into_response(),
        Err(e) => error_response::<TopicDto>(&e),
    }
}

/// Response to voting on an argument: the updated argument and topic.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CommentVoteDto {
    /// The argument with fresh counters.
    pub comment: ForumCommentDto,
    /// The topic with fresh counters/interactions.
    pub topic: TopicDto,
}

/// `POST /f/comments/{id}/vote` — stance on an argument (StackOverflow style;
/// upsert; LOCAL citizens only, by construction).
async fn vote_comment(
    State(state): State<dsoc_app::AppState>,
    caller: dsoc_app::CallerId,
    Path(id): Path<Uuid>,
    Json(req): Json<VoteRequest>,
) -> Response {
    if let Err(e) = state
        .authz
        .require(caller.org, caller.citizen, VerificationLevel::Email)
        .await
    {
        return error_response::<CommentVoteDto>(&e);
    }
    let stance = match req.stance() {
        Ok(s) => s,
        Err(e) => return error_response::<CommentVoteDto>(&e),
    };
    let svc = ForumService::from_state(&state);
    match svc.vote_comment(id, caller.citizen, stance).await {
        Ok((comment, topic)) => {
            // TODO(D5/D6): secondary exposure path — in a small municipality,
            // the vote response still returns the argument's stance/author. The aggregate
            // rule is applied on the public listing (`get_topic`); hardening it here
            // requires resolving the territory on this path (documented groundwork).
            let dto = CommentVoteDto {
                comment: comment_dto(comment, false),
                topic: topic_dto(topic),
            };
            (StatusCode::OK, Json(ApiResponse::ok(dto))).into_response()
        }
        Err(e) => error_response::<CommentVoteDto>(&e),
    }
}

/// Canonical error envelope (never leaks internals).
fn error_response<T: Serialize>(err: &Error) -> Response {
    let status = match err {
        Error::NotFound(_) => StatusCode::NOT_FOUND,
        Error::Forbidden(_) => StatusCode::FORBIDDEN,
        Error::Unauthorized => StatusCode::UNAUTHORIZED,
        Error::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
        Error::Conflict(_) => StatusCode::CONFLICT,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let body: ApiResponse<T> = ApiResponse::fail(err.code(), err.to_string());
    (status, Json(body)).into_response()
}

//! Superfície axum dos fóruns. Mutação exige caller autenticado e verificado
//! (nível Email — o CPF já é validado no cadastro); leitura é pública. Envelope
//! [`ApiResponse`] e mapa de erros idênticos aos demais crates (ADR-0007).

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

/// Organização única da instância (mesma convenção do gateway single-org).
const DEFAULT_ORG_UUID: Uuid = uuid::uuid!("11111111-1111-1111-1111-111111111111");

/// Visão pública de um fórum.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ForumDto {
    /// Id.
    pub id: Uuid,
    /// Caminho completo (`sp/santos/saude`).
    pub full_path: String,
    /// Segmento.
    pub slug: String,
    /// Nome de exibição.
    pub name: String,
    /// Descrição.
    pub description: String,
    /// `institucional` | `governanca` | `comunitario`.
    pub kind: String,
    /// Esfera, se territorial.
    pub esfera: Option<String>,
    /// UF, se territorial.
    pub uf: Option<String>,
    /// Município, se territorial.
    pub municipio: Option<String>,
    /// Há e-mail institucional vinculado (o endereço em si não é exposto).
    pub has_contact_email: bool,
    /// Logo do fórum (0543).
    pub avatar_url: Option<String>,
    /// Capa do fórum (0543).
    pub banner_url: Option<String>,
    /// Patamares de envio configurados.
    pub thresholds: Vec<i32>,
}

/// Um filho na árvore (materializado ou seção virtual do template).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ForumChildDto {
    /// Segmento.
    pub slug: String,
    /// Caminho completo.
    pub full_path: String,
    /// Nome.
    pub name: String,
    /// Seção padrão ainda sem tópicos (materializa no primeiro uso).
    pub virtual_section: bool,
}

/// Resposta da árvore de um nível.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ForumTreeDto {
    /// O fórum pedido (ausente na raiz).
    pub forum: Option<ForumDto>,
    /// Filhos (reais + virtuais).
    pub children: Vec<ForumChildDto>,
}

/// Visão pública de um tópico.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TopicDto {
    /// Id.
    pub id: Uuid,
    /// Fórum dono.
    pub forum_id: Uuid,
    /// Título.
    pub title: String,
    /// Corpo.
    pub body: String,
    /// Handle público opaco do autor (`u-<hex>`).
    pub author_public_handle: String,
    /// Interações CONTÁVEIS (votos + comentários locais) — disparam patamares.
    pub interactions: i64,
    /// Interações FEDERADAS (nunca disparam).
    pub federated_interactions: i64,
    /// Saldo favor - contra.
    pub score: i64,
    /// Posições a favor (fusão debates→fóruns, 0544).
    pub favor: i64,
    /// Posições contra.
    pub contra: i64,
    /// Ponderações.
    pub ponderacao: i64,
    /// Comentários aprovados.
    pub comment_count: i64,
    /// Criação.
    pub created_at: DateTime<Utc>,
}

/// Um comentário (local ou federado).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ForumCommentDto {
    /// Id.
    pub id: Uuid,
    /// Autor local (handle opaco) OU handle remoto.
    pub author: String,
    /// Veio do fediverso?
    pub federated: bool,
    /// Posição declarada com o argumento (`favor`|`contra`|`ponderacao`|null).
    pub stance: Option<String>,
    /// Corpo.
    pub body: String,
    /// Criação.
    pub created_at: DateTime<Utc>,
}

/// Recibo público de envio institucional por patamar.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DispatchDto {
    /// Patamar cruzado.
    pub threshold: i32,
    /// Quando o e-mail saiu do relay (None = na fila).
    pub sent_at: Option<DateTime<Utc>>,
    /// Momento do cruzamento.
    pub crossed_at: DateTime<Utc>,
}

/// Detalhe: tópico + comentários + recibos.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TopicDetailDto {
    /// O tópico.
    pub topic: TopicDto,
    /// Comentários aprovados (locais + federados).
    pub comments: Vec<ForumCommentDto>,
    /// Recibos de envio institucional.
    pub dispatches: Vec<DispatchDto>,
}

/// Criação de tópico.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateTopicRequest {
    /// Caminho do fórum (`sp/santos/saude`).
    pub path: String,
    /// Título.
    pub title: String,
    /// Corpo.
    pub body: String,
}

/// Posição do cidadão (fusão debates→fóruns).
#[derive(Debug, Deserialize, ToSchema)]
pub struct VoteRequest {
    /// `favor` | `contra` | `ponderacao`.
    pub stance: Option<String>,
    /// Compat com clientes antigos: +1 → favor, -1 → contra.
    pub value: Option<i16>,
}

impl VoteRequest {
    /// Resolve a posição pedida (stance preferido; `value` legado mapeado).
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

/// Comentário local (argumento) — com posição opcional que também registra o voto.
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

fn comment_dto(r: CommentRow) -> ForumCommentDto {
    let author = match (&r.author_id, &r.remote_handle) {
        (Some(cid), _) => format!("u-{}", cid.as_simple()),
        (None, Some(h)) => h.clone(),
        (None, None) => "(desconhecido)".to_owned(),
    };
    ForumCommentDto {
        id: r.id,
        author,
        federated: r.federated,
        stance: r.stance,
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

/// Router do crate — montado pelo gateway sob `/api/v1`.
pub fn routes(state: dsoc_app::AppState) -> Router<()> {
    Router::new()
        .route("/f/tree", get(tree))
        .route("/f/recent", get(recent))
        .route("/f/topics", post(create_topic).get(list_topics))
        .route("/f/topics/{id}", get(get_topic))
        .route("/f/topics/{id}/vote", post(vote))
        .route("/f/topics/{id}/comments", post(comment))
        .with_state(state)
}

/// Um item do feed de últimas postagens (home /f).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RecentTopicDto {
    /// Id do tópico.
    pub id: Uuid,
    /// Título.
    pub title: String,
    /// Saldo favor - contra.
    pub score: i64,
    /// Posições a favor.
    pub favor: i64,
    /// Posições contra.
    pub contra: i64,
    /// Ponderações.
    pub ponderacao: i64,
    /// Interações contáveis.
    pub interactions: i64,
    /// Comentários.
    pub comment_count: i64,
    /// Criação.
    pub created_at: DateTime<Utc>,
    /// Caminho do fórum.
    pub forum_path: String,
    /// Nome do fórum.
    pub forum_name: String,
}

/// `GET /f/recent?limit=` — últimas postagens de todos os fóruns (feed da home).
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

/// `GET /f/tree?path=&esfera=` — um nível da árvore (raiz quando sem `path`).
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

/// `POST /f/topics` — criar tópico (cidadão verificado; CPF já validado no cadastro).
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
        .create_topic(caller.org, &req.path, caller.citizen, &new)
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
            let dto = serde_json::json!({
                "forum": forum_dto(forum),
                "topics": topics.into_iter().map(topic_dto).collect::<Vec<_>>(),
            });
            (StatusCode::OK, Json(ApiResponse::ok(dto))).into_response()
        }
        Err(e) => error_response::<serde_json::Value>(&e),
    }
}

/// `GET /f/topics/{id}` — detalhe com comentários e recibos.
async fn get_topic(State(state): State<dsoc_app::AppState>, Path(id): Path<Uuid>) -> Response {
    let svc = ForumService::from_state(&state);
    match svc.get_topic(id).await {
        Ok((topic, comments, dispatches)) => {
            let dto = TopicDetailDto {
                topic: topic_dto(topic),
                comments: comments.into_iter().map(comment_dto).collect(),
                dispatches: dispatches.into_iter().map(dispatch_dto).collect(),
            };
            (StatusCode::OK, Json(ApiResponse::ok(dto))).into_response()
        }
        Err(e) => error_response::<TopicDetailDto>(&e),
    }
}

/// `POST /f/topics/{id}/vote` — posição a favor/contra/ponderação (upsert;
/// SÓ cidadão local, por construção).
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

/// `POST /f/topics/{id}/comments` — comentário local.
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

/// Envelope de erro canônico (não vaza internals).
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

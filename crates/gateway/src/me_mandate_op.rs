//! # Orçamento participativo — piloto de MANDATO (D8.3).
//!
//! O salto de "medir raiva" para "exercer poder": a base decide onde vai uma
//! fatia REAL de verba. Piloto viável = **verba de emenda de um mandato** (um
//! vereador/deputado aliado), NÃO a prefeitura — evita o ciclo institucional
//! longo. Referência: Orçamento Participativo de Porto Alegre. A copy em toda
//! superfície é honesta: *"piloto — verba de emenda do mandato"*.
//!
//! O que separa OP de "mais uma enquete" é a **prestação de contas**: depois da
//! votação, cada item ganha um `execution_status` (previsto → em_andamento →
//! concluído / não executado) que fecha o loop de poder.
//!
//! ## Ciclo de uma rodada (`op_round.phase`)
//! `propostas` → `votacao` → `resultado` → `execucao`
//!
//! ## Gate do operador
//! Mesmíssimo critério de `campanha.rs` / `me_mandate_crm.rs`: quem tem vínculo
//! em `mandate_identity_binding`. O vínculo é a autorização E a chave de escopo —
//! um operador só mexe nas rodadas do PRÓPRIO mandato (todo UPDATE chaveia por
//! `op_round.mandate_id = <mandato do caller>`).
//!
//! ## Endpoints
//! Operador (gate):
//! - `POST /me/mandate/op/rounds` — cria rodada (title, budget, território).
//! - `POST /me/mandate/op/rounds/{id}/phase` — avança fase.
//! - `POST /me/mandate/op/rounds/{id}/items/{item}/execution` — marca execução.
//!
//! Cidadão logado:
//! - `POST /op/rounds/{id}/items` — submete item (só na fase `propostas`).
//! - `POST /op/rounds/{id}/vote` — vota num item (só na fase `votacao`), upsert
//!   1 voto por rodada.
//!
//! Público:
//! - `GET /op/rounds/{id}` — rodada + itens + contagem de votos + ranking dentro
//!   do orçamento.
//! - `GET /politicos/{mandate_id}/op` — rodadas do mandato.
//!
//! ## Nota LGPD
//! A superfície pública só expõe **autoria pública** de itens (mesmo princípio
//! da autoria de proposta). Nunca expõe quem votou em quê — o voto por-cidadão
//! fica em `op_vote` e só alimenta a CONTAGEM agregada.

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

const MAX_TITLE: usize = 160;
const MAX_DESCRIPTION: usize = 2000;
/// Teto de sanidade da verba: R$ 100 milhões em centavos. Emenda de mandato
/// nunca chega perto — mas barra digitação com zeros a mais.
const MAX_BUDGET_CENTS: i64 = 10_000_000_000;
/// Teto de itens lidos por rodada — a superfície pública é um resumo.
const ITEMS_LIMIT: i64 = 500;
const ROUNDS_LIMIT: i64 = 100;

const PHASES: [&str; 4] = ["propostas", "votacao", "resultado", "execucao"];
const EXECUTION_STATUSES: [&str; 4] =
    ["previsto", "em_andamento", "concluido", "nao_executado"];

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/me/mandate/op/rounds", post(create_round))
        .route("/me/mandate/op/rounds/{id}/phase", post(advance_phase))
        .route(
            "/me/mandate/op/rounds/{id}/items/{item}/execution",
            post(mark_execution),
        )
        .route("/op/rounds/{id}/items", post(submit_item))
        .route("/op/rounds/{id}/vote", post(cast_vote))
        .route("/op/rounds", get(recent_rounds))
        .route("/op/rounds/{id}", get(public_round))
        .route("/politicos/{mandate_id}/op", get(mandate_rounds))
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

fn storage_error() -> Response {
    fail(
        StatusCode::INTERNAL_SERVER_ERROR,
        "storage_error",
        "Erro interno.",
    )
}

fn unauthorized() -> Response {
    fail(
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "Autenticação necessária.",
    )
}

fn not_operator() -> Response {
    fail(
        StatusCode::FORBIDDEN,
        "not_operator",
        "O orçamento participativo é exclusivo de contas vinculadas a um mandato.",
    )
}

fn ok_json<T: Serialize>(data: T) -> Response {
    (StatusCode::OK, Json(ApiResponse::ok(data))).into_response()
}

/// Gate + escopo: resolve o mandato do caller pelo vínculo. `None` = sem vínculo.
async fn caller_mandate(db: &PgPool, citizen: Uuid) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT mandate_id FROM mandate_identity_binding \
         WHERE citizen_id = $1 ORDER BY verified_at DESC LIMIT 1",
    )
    .bind(citizen)
    .fetch_optional(db)
    .await
}

// ---------------------------------------------------------------------------
// Ranking dentro do orçamento (PURO — testável sem DB)
// ---------------------------------------------------------------------------

/// Entrada mínima do ranqueador: votos e custo estimado (centavos, opcional).
#[derive(Debug, Clone, Copy)]
struct RankInput {
    votes: i64,
    estimated_cents: Option<i64>,
}

/// Saída por item: posição no ranking (1-based) e se CABE na verba acumulada.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RankOutput {
    rank: i32,
    fits: bool,
}

/// **Ranking dentro do orçamento** (o coração do "vencedor = conjunto que cabe").
///
/// Ordena por votos desc (empate → ordem de entrada, estável) e, guloso, marca
/// `fits=true` enquanto o custo acumulado dos itens que cabem não estoura o
/// `budget_cents`. Item sem estimativa (`None`) nunca "cabe" — não dá pra
/// prometer o que não se sabe custar — mas ainda recebe posição no ranking.
///
/// Retorna um vetor alinhado ao índice de entrada (não à ordem de votos), pra o
/// chamador só casar de volta nos itens originais.
fn rank_within_budget(items: &[RankInput], budget_cents: i64) -> Vec<RankOutput> {
    // Índices ordenados por votos desc, empate por ordem original (estável).
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|&a, &b| items[b].votes.cmp(&items[a].votes).then_with(|| a.cmp(&b)));

    let mut out = vec![RankOutput { rank: 0, fits: false }; items.len()];
    let mut spent: i64 = 0;
    for (pos, &idx) in order.iter().enumerate() {
        let rank = (pos as i32) + 1;
        let fits = match items[idx].estimated_cents {
            Some(cost) if cost >= 0 && spent.saturating_add(cost) <= budget_cents => {
                spent = spent.saturating_add(cost);
                true
            }
            _ => false,
        };
        out[idx] = RankOutput { rank, fits };
    }
    out
}

// ---------------------------------------------------------------------------
// POST /me/mandate/op/rounds — operador cria uma rodada
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CreateRoundBody {
    title: String,
    budget_cents: i64,
    #[serde(default)]
    uf: Option<String>,
    #[serde(default)]
    municipio_ibge: Option<i32>,
}

async fn create_round(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateRoundBody>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let mandate_id = match caller_mandate(&state.db, citizen).await {
        Ok(Some(m)) => m,
        Ok(None) => return not_operator(),
        Err(err) => {
            tracing::error!(?err, "op create_round gate");
            return storage_error();
        }
    };
    let title = body.title.trim();
    if title.is_empty() || title.len() > MAX_TITLE {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_title",
            "Título obrigatório, até 160 caracteres.",
        );
    }
    if body.budget_cents <= 0 || body.budget_cents > MAX_BUDGET_CENTS {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_budget",
            "A verba deve ser positiva (em centavos) e dentro do teto.",
        );
    }
    let uf = match body.uf.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(v) if v.len() == 2 => Some(v.to_uppercase()),
        Some(_) => {
            return fail(StatusCode::BAD_REQUEST, "invalid_uf", "UF deve ter 2 letras.")
        }
    };
    let id = Uuid::now_v7();
    let res = sqlx::query(
        "INSERT INTO op_round (id, mandate_id, title, budget_cents, uf, municipio_ibge) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(mandate_id)
    .bind(title)
    .bind(body.budget_cents)
    .bind(uf.as_deref())
    .bind(body.municipio_ibge)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => ok_json(serde_json::json!({ "id": id })),
        Err(err) => {
            tracing::error!(?err, "op round insert");
            storage_error()
        }
    }
}

// ---------------------------------------------------------------------------
// POST /me/mandate/op/rounds/{id}/phase — operador avança a fase
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct PhaseBody {
    phase: String,
}

async fn advance_phase(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<PhaseBody>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let mandate_id = match caller_mandate(&state.db, citizen).await {
        Ok(Some(m)) => m,
        Ok(None) => return not_operator(),
        Err(err) => {
            tracing::error!(?err, "op advance_phase gate");
            return storage_error();
        }
    };
    if !PHASES.contains(&body.phase.as_str()) {
        return fail(StatusCode::BAD_REQUEST, "invalid_phase", "Fase inválida.");
    }
    // Escopo: só rodada DESTE mandato. rows_affected=0 ⇒ não é sua (ou não existe).
    let res = sqlx::query("UPDATE op_round SET phase = $1 WHERE id = $2 AND mandate_id = $3")
        .bind(&body.phase)
        .bind(id)
        .bind(mandate_id)
        .execute(&state.db)
        .await;
    match res {
        Ok(r) if r.rows_affected() > 0 => ok_json(serde_json::json!({ "phase": body.phase })),
        Ok(_) => fail(StatusCode::NOT_FOUND, "not_found", "Rodada não encontrada."),
        Err(err) => {
            tracing::error!(?err, "op phase update");
            storage_error()
        }
    }
}

// ---------------------------------------------------------------------------
// POST /me/mandate/op/rounds/{id}/items/{item}/execution — prestação de contas
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ExecutionBody {
    execution_status: String,
}

async fn mark_execution(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, item)): Path<(Uuid, Uuid)>,
    Json(body): Json<ExecutionBody>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let mandate_id = match caller_mandate(&state.db, citizen).await {
        Ok(Some(m)) => m,
        Ok(None) => return not_operator(),
        Err(err) => {
            tracing::error!(?err, "op mark_execution gate");
            return storage_error();
        }
    };
    if !EXECUTION_STATUSES.contains(&body.execution_status.as_str()) {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_status",
            "Status de execução inválido.",
        );
    }
    // Escopo duplo: o item precisa ser da rodada {id} E a rodada ser do mandato
    // do caller. A subquery garante que operador de outro gabinete não marca aqui.
    let res = sqlx::query(
        "UPDATE op_item SET execution_status = $1 \
         WHERE id = $2 AND round_id = $3 \
           AND round_id IN (SELECT id FROM op_round WHERE mandate_id = $4)",
    )
    .bind(&body.execution_status)
    .bind(item)
    .bind(id)
    .bind(mandate_id)
    .execute(&state.db)
    .await;
    match res {
        Ok(r) if r.rows_affected() > 0 => {
            ok_json(serde_json::json!({ "execution_status": body.execution_status }))
        }
        Ok(_) => fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "Item não encontrado nesta rodada.",
        ),
        Err(err) => {
            tracing::error!(?err, "op execution update");
            storage_error()
        }
    }
}

// ---------------------------------------------------------------------------
// POST /op/rounds/{id}/items — cidadão logado submete um item (fase propostas)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SubmitItemBody {
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    estimated_cents: Option<i64>,
}

async fn submit_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<SubmitItemBody>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let title = body.title.trim();
    if title.is_empty() || title.len() > MAX_TITLE {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_title",
            "Título obrigatório, até 160 caracteres.",
        );
    }
    let description = body.description.as_deref().unwrap_or("").trim();
    if description.len() > MAX_DESCRIPTION {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_description",
            "Descrição tem limite de 2000 caracteres.",
        );
    }
    if let Some(cost) = body.estimated_cents {
        if cost < 0 || cost > MAX_BUDGET_CENTS {
            return fail(
                StatusCode::BAD_REQUEST,
                "invalid_estimate",
                "Custo estimado deve ser não-negativo e dentro do teto.",
            );
        }
    }
    // A rodada precisa existir E estar na fase 'propostas'.
    let phase: Option<String> = match sqlx::query_scalar("SELECT phase FROM op_round WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "op submit_item phase read");
            return storage_error();
        }
    };
    match phase.as_deref() {
        None => return fail(StatusCode::NOT_FOUND, "not_found", "Rodada não encontrada."),
        Some("propostas") => {}
        Some(_) => {
            return fail(
                StatusCode::CONFLICT,
                "wrong_phase",
                "Esta rodada não está aberta para propostas.",
            )
        }
    }
    let item_id = Uuid::now_v7();
    let res = sqlx::query(
        "INSERT INTO op_item (id, round_id, author_citizen_id, title, description, estimated_cents) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(item_id)
    .bind(id)
    .bind(citizen)
    .bind(title)
    .bind(description)
    .bind(body.estimated_cents)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => ok_json(serde_json::json!({ "id": item_id })),
        Err(err) => {
            tracing::error!(?err, "op item insert");
            storage_error()
        }
    }
}

// ---------------------------------------------------------------------------
// POST /op/rounds/{id}/vote — cidadão logado vota (fase votacao), 1 por rodada
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct VoteBody {
    item_id: Uuid,
}

async fn cast_vote(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<VoteBody>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let phase: Option<String> = match sqlx::query_scalar("SELECT phase FROM op_round WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "op cast_vote phase read");
            return storage_error();
        }
    };
    match phase.as_deref() {
        None => return fail(StatusCode::NOT_FOUND, "not_found", "Rodada não encontrada."),
        Some("votacao") => {}
        Some(_) => {
            return fail(
                StatusCode::CONFLICT,
                "wrong_phase",
                "Esta rodada não está em votação.",
            )
        }
    }
    // O item precisa pertencer a ESTA rodada (evita votar em item de outra).
    let belongs: bool = match sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM op_item WHERE id = $1 AND round_id = $2)",
    )
    .bind(body.item_id)
    .bind(id)
    .fetch_one(&state.db)
    .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "op cast_vote item check");
            return storage_error();
        }
    };
    if !belongs {
        return fail(
            StatusCode::NOT_FOUND,
            "item_not_found",
            "Item não encontrado nesta rodada.",
        );
    }
    // Upsert: 1 voto por (rodada, cidadão). Trocar de item sobrescreve.
    let res = sqlx::query(
        "INSERT INTO op_vote (round_id, item_id, citizen_id) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (round_id, citizen_id) \
         DO UPDATE SET item_id = EXCLUDED.item_id, created_at = now()",
    )
    .bind(id)
    .bind(body.item_id)
    .bind(citizen)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => ok_json(serde_json::json!({ "voted": true, "item_id": body.item_id })),
        Err(err) => {
            tracing::error!(?err, "op vote upsert");
            storage_error()
        }
    }
}

// ---------------------------------------------------------------------------
// GET /op/rounds/{id} — superfície pública (rodada + itens + ranking)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct PublicItemDto {
    id: Uuid,
    title: String,
    description: String,
    estimated_cents: Option<i64>,
    votes: i64,
    /// Autoria pública (handle) — `None` se item do gabinete/anônimo.
    author_handle: Option<String>,
    author_display_name: Option<String>,
    execution_status: Option<String>,
    /// Posição no ranking por votos (1-based). Só significativo em resultado/execução.
    rank: i32,
    /// Cabe na verba acumulada (o "vencedor" é o conjunto que cabe).
    fits_budget: bool,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct PublicRoundDto {
    id: Uuid,
    mandate_id: Uuid,
    mandate_name: Option<String>,
    title: String,
    budget_cents: i64,
    uf: Option<String>,
    municipio_ibge: Option<i32>,
    phase: String,
    total_votes: i64,
    /// Soma dos custos dos itens que cabem (o compromisso do conjunto vencedor).
    allocated_cents: i64,
    items: Vec<PublicItemDto>,
    created_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct RoundRow {
    id: Uuid,
    mandate_id: Uuid,
    mandate_name: Option<String>,
    title: String,
    budget_cents: i64,
    uf: Option<String>,
    municipio_ibge: Option<i32>,
    phase: String,
    created_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct ItemRow {
    id: Uuid,
    title: String,
    description: String,
    estimated_cents: Option<i64>,
    votes: i64,
    author_handle: Option<String>,
    author_display_name: Option<String>,
    execution_status: Option<String>,
    created_at: DateTime<Utc>,
}

async fn public_round(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    let round: Option<RoundRow> = match sqlx::query_as(
        "SELECT r.id, r.mandate_id, m.display_name AS mandate_name, r.title, \
                r.budget_cents, r.uf, r.municipio_ibge, r.phase, r.created_at \
           FROM op_round r \
           LEFT JOIN mandate m ON m.id = r.mandate_id \
          WHERE r.id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "op public_round read");
            return storage_error();
        }
    };
    let Some(round) = round else {
        return fail(StatusCode::NOT_FOUND, "not_found", "Rodada não encontrada.");
    };
    let rows: Vec<ItemRow> = match sqlx::query_as(
        "SELECT i.id, i.title, i.description, i.estimated_cents, \
                (SELECT count(*) FROM op_vote v WHERE v.item_id = i.id) AS votes, \
                c.handle AS author_handle, c.display_name AS author_display_name, \
                i.execution_status, i.created_at \
           FROM op_item i \
           LEFT JOIN citizen c ON c.id = i.author_citizen_id \
          WHERE i.round_id = $1 \
          ORDER BY i.created_at ASC \
          LIMIT $2",
    )
    .bind(id)
    .bind(ITEMS_LIMIT)
    .fetch_all(&state.db)
    .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "op public_round items read");
            return storage_error();
        }
    };

    let ranks = rank_within_budget(
        &rows
            .iter()
            .map(|r| RankInput {
                votes: r.votes,
                estimated_cents: r.estimated_cents,
            })
            .collect::<Vec<_>>(),
        round.budget_cents,
    );
    let total_votes: i64 = rows.iter().map(|r| r.votes).sum();
    let allocated_cents: i64 = rows
        .iter()
        .zip(ranks.iter())
        .filter(|(_, rk)| rk.fits)
        .filter_map(|(r, _)| r.estimated_cents)
        .sum();

    let items: Vec<PublicItemDto> = rows
        .into_iter()
        .zip(ranks)
        .map(|(r, rk)| PublicItemDto {
            id: r.id,
            title: r.title,
            description: r.description,
            estimated_cents: r.estimated_cents,
            votes: r.votes,
            author_handle: r.author_handle,
            author_display_name: r.author_display_name,
            execution_status: r.execution_status,
            rank: rk.rank,
            fits_budget: rk.fits,
            created_at: r.created_at,
        })
        .collect();

    ok_json(PublicRoundDto {
        id: round.id,
        mandate_id: round.mandate_id,
        mandate_name: round.mandate_name,
        title: round.title,
        budget_cents: round.budget_cents,
        uf: round.uf,
        municipio_ibge: round.municipio_ibge,
        phase: round.phase,
        total_votes,
        allocated_cents,
        items,
        created_at: round.created_at,
    })
}

// ---------------------------------------------------------------------------
// GET /politicos/{mandate_id}/op — rodadas de um mandato (superfície pública)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, sqlx::FromRow)]
struct RoundSummaryDto {
    id: Uuid,
    title: String,
    budget_cents: i64,
    uf: Option<String>,
    municipio_ibge: Option<i32>,
    phase: String,
    items_count: i64,
    total_votes: i64,
    created_at: DateTime<Utc>,
}

/// `GET /op/rounds` — rodadas recentes de todos os mandatos (descoberta + SSG).
/// Público: só metadados agregados, sem PII.
async fn recent_rounds(State(state): State<AppState>) -> Response {
    let rounds: Vec<RoundSummaryDto> = match sqlx::query_as(
        "SELECT r.id, r.title, r.budget_cents, r.uf, r.municipio_ibge, r.phase, \
                (SELECT count(*) FROM op_item i WHERE i.round_id = r.id) AS items_count, \
                (SELECT count(*) FROM op_vote v WHERE v.round_id = r.id) AS total_votes, \
                r.created_at \
           FROM op_round r \
          ORDER BY r.created_at DESC \
          LIMIT $1",
    )
    .bind(ROUNDS_LIMIT)
    .fetch_all(&state.db)
    .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "op recent_rounds read");
            return storage_error();
        }
    };
    ok_json(serde_json::json!({ "rounds": rounds }))
}

async fn mandate_rounds(State(state): State<AppState>, Path(mandate_id): Path<Uuid>) -> Response {
    let rounds: Vec<RoundSummaryDto> = match sqlx::query_as(
        "SELECT r.id, r.title, r.budget_cents, r.uf, r.municipio_ibge, r.phase, \
                (SELECT count(*) FROM op_item i WHERE i.round_id = r.id) AS items_count, \
                (SELECT count(*) FROM op_vote v WHERE v.round_id = r.id) AS total_votes, \
                r.created_at \
           FROM op_round r \
          WHERE r.mandate_id = $1 \
          ORDER BY r.created_at DESC \
          LIMIT $2",
    )
    .bind(mandate_id)
    .bind(ROUNDS_LIMIT)
    .fetch_all(&state.db)
    .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "op mandate_rounds read");
            return storage_error();
        }
    };
    ok_json(serde_json::json!({ "mandate_id": mandate_id, "rounds": rounds }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(votes: i64, cost: Option<i64>) -> RankInput {
        RankInput {
            votes,
            estimated_cents: cost,
        }
    }

    #[test]
    fn ranks_by_votes_desc_stable_on_ties() {
        // b tem mais votos; a e c empatam → mantêm ordem de entrada (a antes de c).
        let items = [item(3, Some(100)), item(5, Some(100)), item(3, Some(100))];
        let out = rank_within_budget(&items, 1_000);
        assert_eq!(out[1].rank, 1); // b (5 votos) é o 1º
        assert_eq!(out[0].rank, 2); // a (3, entrou antes)
        assert_eq!(out[2].rank, 3); // c (3, entrou depois)
    }

    #[test]
    fn fits_marks_the_winning_set_within_budget() {
        // Orçamento 250. Por votos desc: a(300,cost150), b(200,cost150), c(100,cost90).
        // a cabe (150 ≤ 250). b NÃO cabe (150+150=300 > 250). c cabe (150+90=240 ≤ 250).
        let items = [item(300, Some(150)), item(200, Some(150)), item(100, Some(90))];
        let out = rank_within_budget(&items, 250);
        assert!(out[0].fits, "a deve caber");
        assert!(!out[1].fits, "b não deve caber");
        assert!(out[2].fits, "c deve caber (pula o que não coube)");
        // Acumulado dos que cabem = 150 + 90 = 240.
        let allocated: i64 = items
            .iter()
            .zip(out.iter())
            .filter(|(_, rk)| rk.fits)
            .filter_map(|(it, _)| it.estimated_cents)
            .sum();
        assert_eq!(allocated, 240);
    }

    #[test]
    fn item_without_estimate_never_fits_but_is_ranked() {
        let items = [item(10, None), item(5, Some(50))];
        let out = rank_within_budget(&items, 1_000);
        assert_eq!(out[0].rank, 1);
        assert!(!out[0].fits, "sem estimativa não cabe");
        assert_eq!(out[1].rank, 2);
        assert!(out[1].fits, "com estimativa dentro do teto cabe");
    }

    #[test]
    fn zero_budget_fits_nothing_with_cost() {
        let items = [item(10, Some(1)), item(5, Some(0))];
        let out = rank_within_budget(&items, 0);
        assert!(!out[0].fits, "custo 1 não cabe em 0");
        assert!(out[1].fits, "custo 0 cabe em 0");
    }

    #[test]
    fn empty_input_is_empty_output() {
        let out = rank_within_budget(&[], 100);
        assert!(out.is_empty());
    }
}

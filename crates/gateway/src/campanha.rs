//! # Doações e financiamento de campanha (0.31.0, migration 0523).
//!
//! Serviço da página `/servicos`: o(a) político(a) declara o financiamento
//! da campanha (entradas/saídas, histórico imutável — corrige revogando) e
//! configura a página de arrecadação (meta, meios oficiais, publicação).
//!
//! **Gate**: só cidadão "do tipo político" — quem tem vínculo de mandato em
//! `mandate_identity_binding` (mesmo critério do painel-mandato). Todos os
//! endpoints exigem sessão; os de escrita exigem o gate.
//!
//! - `GET    /me/campanha` — visão completa: flag do gate, config e lançamentos.
//! - `POST   /me/campanha/lancamentos` — novo lançamento (entrada/saída).
//! - `DELETE /me/campanha/lancamentos/{id}` — revoga o próprio lançamento.
//! - `PUT    /me/campanha/config` — upsert da configuração de arrecadação.
//! - `GET    /campanha/{handle}` — página PÚBLICA da declaração (só quando
//!   `is_published` e o vínculo de mandato segue vivo; 404 uniforme nos demais
//!   casos pra não vazar existência de config despublicada).

use axum::extract::{Json, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::Router;
use chrono::{DateTime, NaiveDate, Utc};
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use dsoc_auth::profile::ProfileService;
use dsoc_core::ids::OrgId;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

/// Mesma org default fixa da superfície de federação/perfis públicos.
const DEFAULT_ORG_UUID: Uuid = uuid::uuid!("11111111-1111-1111-1111-111111111111");

const MAX_DESCRICAO: usize = 200;
const MAX_RECEIPT: usize = 60;
const MAX_DONOR: usize = 120;
const MAX_BANK: usize = 200;
const MAX_URL: usize = 300;
/// Teto sanidade por lançamento: R$ 100 milhões em centavos. O TSE nunca
/// viu campanha municipal ou federal perto disso num lançamento só.
const MAX_VALOR_CENTAVOS: i64 = 10_000_000_000;
const LIST_LIMIT: i64 = 500;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/me/campanha", get(overview))
        .route("/me/campanha/lancamentos", axum::routing::post(add_entry))
        .route(
            "/me/campanha/lancamentos/{id}",
            axum::routing::delete(revoke_entry),
        )
        .route("/me/campanha/config", put(save_config))
        .route("/campanha/{handle}", get(public_view))
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

fn not_politico() -> Response {
    fail(
        StatusCode::FORBIDDEN,
        "not_politico",
        "O serviço de campanha é exclusivo de contas vinculadas a mandato.",
    )
}

/// Gate "político": o mesmo vínculo que abre o painel-mandato.
async fn is_politico(db: &PgPool, citizen: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        r"SELECT EXISTS (SELECT 1 FROM mandate_identity_binding WHERE citizen_id = $1)",
    )
    .bind(citizen)
    .fetch_one(db)
    .await
}

/// Vínculo VERIFICADO (directory/strong)? Candidato auto-declarado (binding
/// nível 'email', 0526) fica `false` — o front pinta o selo "candidatura
/// autodeclarada — não verificada" nas superfícies públicas.
async fn is_verificado(db: &PgPool, citizen: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        r"SELECT EXISTS (SELECT 1 FROM mandate_identity_binding
           WHERE citizen_id = $1 AND verification_level IN ('directory','strong'))",
    )
    .bind(citizen)
    .fetch_one(db)
    .await
}

// ---------------------------------------------------------------------------
// GET /me/campanha — visão completa
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, sqlx::FromRow)]
struct EntryDto {
    id: Uuid,
    kind: String,
    descricao: String,
    valor_centavos: i64,
    occurred_on: NaiveDate,
    receipt_ref: Option<String>,
    donor_name: Option<String>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct ConfigDto {
    meta_centavos: Option<i64>,
    bank_account: Option<String>,
    crowdfunding_url: Option<String>,
    is_published: bool,
}

#[derive(Debug, Serialize)]
struct CampanhaDto {
    is_politico: bool,
    /// Vínculo directory/strong. false = candidatura autodeclarada (selo no front).
    verificado: bool,
    config: Option<ConfigDto>,
    lancamentos: Vec<EntryDto>,
}

async fn overview(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let politico = match is_politico(&state.db, citizen).await {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "campanha gate check");
            return storage_error();
        }
    };
    if !politico {
        // O front (e o AuthMenu) usa a flag pra esconder o serviço — 200 com
        // is_politico=false em vez de 403 pra não poluir o console de quem
        // só abriu o menu.
        return (
            StatusCode::OK,
            Json(ApiResponse::ok(CampanhaDto {
                is_politico: false,
                verificado: false,
                config: None,
                lancamentos: Vec::new(),
            })),
        )
            .into_response();
    }
    let verificado = match is_verificado(&state.db, citizen).await {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "campanha verificado check");
            return storage_error();
        }
    };
    let config: Option<ConfigDto> = match sqlx::query_as(
        r"SELECT meta_centavos, bank_account, crowdfunding_url, is_published
            FROM campaign_fundraising_config
           WHERE citizen_id = $1",
    )
    .bind(citizen)
    .fetch_optional(&state.db)
    .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "campanha config read");
            return storage_error();
        }
    };
    let lancamentos: Vec<EntryDto> = match sqlx::query_as(
        r"SELECT id, kind, descricao, valor_centavos, occurred_on,
                 receipt_ref, donor_name, created_at
            FROM campaign_finance_entry
           WHERE citizen_id = $1 AND revoked_at IS NULL
           ORDER BY occurred_on DESC, created_at DESC
           LIMIT $2",
    )
    .bind(citizen)
    .bind(LIST_LIMIT)
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(err) => {
            tracing::error!(?err, "campanha entries read");
            return storage_error();
        }
    };
    (
        StatusCode::OK,
        Json(ApiResponse::ok(CampanhaDto {
            is_politico: true,
            verificado,
            config,
            lancamentos,
        })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /campanha/{handle} — página pública da declaração
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct CampanhaPublicaDto {
    handle: String,
    display_name: Option<String>,
    avatar_url: Option<String>,
    /// false = candidatura autodeclarada, ainda sem verificação (selo público).
    verificado: bool,
    meta_centavos: Option<i64>,
    bank_account: Option<String>,
    crowdfunding_url: Option<String>,
    total_entradas_centavos: i64,
    total_saidas_centavos: i64,
    doacoes_count: i64,
    lancamentos: Vec<EntryDto>,
}

fn public_not_found() -> Response {
    // Mensagem única para handle inexistente, config despublicada ou vínculo
    // perdido — quem está de fora não descobre qual dos três.
    fail(
        StatusCode::NOT_FOUND,
        "not_found",
        "Declaração de campanha não encontrada.",
    )
}

async fn public_view(State(state): State<AppState>, Path(handle): Path<String>) -> Response {
    let svc = ProfileService::from_state(&state);
    let org = OrgId::from_uuid(DEFAULT_ORG_UUID);
    let Ok(profile) = svc.find_public_by_handle(org, &handle).await else {
        return public_not_found();
    };
    let citizen = profile.citizen_id;
    let config: Option<ConfigDto> = match sqlx::query_as(
        r"SELECT meta_centavos, bank_account, crowdfunding_url, is_published
            FROM campaign_fundraising_config
           WHERE citizen_id = $1 AND is_published",
    )
    .bind(citizen)
    .fetch_optional(&state.db)
    .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "campanha public config read");
            return storage_error();
        }
    };
    let Some(config) = config else {
        return public_not_found();
    };
    // Vínculo perdido despublica na prática — a página some junto com o gate.
    match is_politico(&state.db, citizen).await {
        Ok(true) => {}
        Ok(false) => return public_not_found(),
        Err(err) => {
            tracing::error!(?err, "campanha public gate check");
            return storage_error();
        }
    }
    let verificado = match is_verificado(&state.db, citizen).await {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(?err, "campanha public verificado check");
            return storage_error();
        }
    };
    let lancamentos: Vec<EntryDto> = match sqlx::query_as(
        r"SELECT id, kind, descricao, valor_centavos, occurred_on,
                 receipt_ref, donor_name, created_at
            FROM campaign_finance_entry
           WHERE citizen_id = $1 AND revoked_at IS NULL
           ORDER BY occurred_on DESC, created_at DESC
           LIMIT $2",
    )
    .bind(citizen)
    .bind(LIST_LIMIT)
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(err) => {
            tracing::error!(?err, "campanha public entries read");
            return storage_error();
        }
    };
    let total_entradas_centavos = lancamentos
        .iter()
        .filter(|l| l.kind == "entrada")
        .map(|l| l.valor_centavos)
        .sum();
    let total_saidas_centavos = lancamentos
        .iter()
        .filter(|l| l.kind == "saida")
        .map(|l| l.valor_centavos)
        .sum();
    let doacoes_count = lancamentos
        .iter()
        .filter(|l| l.kind == "entrada" && l.receipt_ref.is_some())
        .count() as i64;
    (
        StatusCode::OK,
        Json(ApiResponse::ok(CampanhaPublicaDto {
            handle: profile.handle.unwrap_or(profile.public_handle),
            display_name: profile.display_name,
            avatar_url: profile.avatar_url,
            verificado,
            meta_centavos: config.meta_centavos,
            bank_account: config.bank_account,
            crowdfunding_url: config.crowdfunding_url,
            total_entradas_centavos,
            total_saidas_centavos,
            doacoes_count,
            lancamentos,
        })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// POST /me/campanha/lancamentos
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct AddEntryBody {
    kind: String,
    descricao: String,
    valor_centavos: i64,
    occurred_on: NaiveDate,
    #[serde(default)]
    receipt_ref: Option<String>,
    #[serde(default)]
    donor_name: Option<String>,
}

fn trimmed_opt(value: Option<String>, max: usize) -> Result<Option<String>, ()> {
    match value.as_deref().map(str::trim) {
        None => Ok(None),
        Some("") => Ok(None),
        Some(v) if v.len() <= max => Ok(Some(v.to_owned())),
        Some(_) => Err(()),
    }
}

async fn add_entry(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AddEntryBody>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    match is_politico(&state.db, citizen).await {
        Ok(true) => {}
        Ok(false) => return not_politico(),
        Err(err) => {
            tracing::error!(?err, "campanha gate check");
            return storage_error();
        }
    }
    if body.kind != "entrada" && body.kind != "saida" {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_kind",
            "kind deve ser 'entrada' ou 'saida'.",
        );
    }
    let descricao = body.descricao.trim();
    if descricao.is_empty() || descricao.len() > MAX_DESCRICAO {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_descricao",
            "Descrição obrigatória, até 200 caracteres.",
        );
    }
    if body.valor_centavos <= 0 || body.valor_centavos > MAX_VALOR_CENTAVOS {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_valor",
            "Valor deve ser positivo (em centavos) e dentro do teto.",
        );
    }
    let Ok(receipt_ref) = trimmed_opt(body.receipt_ref, MAX_RECEIPT) else {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_receipt",
            "Recibo eleitoral tem limite de 60 caracteres.",
        );
    };
    let Ok(donor_name) = trimmed_opt(body.donor_name, MAX_DONOR) else {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_donor",
            "Nome do(a) doador(a) tem limite de 120 caracteres.",
        );
    };
    if body.kind == "saida" && (receipt_ref.is_some() || donor_name.is_some()) {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_fields",
            "Recibo e doador(a) só valem para entradas.",
        );
    }
    let id = Uuid::now_v7();
    let res = sqlx::query(
        r"INSERT INTO campaign_finance_entry
              (id, citizen_id, kind, descricao, valor_centavos, occurred_on,
               receipt_ref, donor_name)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(citizen)
    .bind(&body.kind)
    .bind(descricao)
    .bind(body.valor_centavos)
    .bind(body.occurred_on)
    .bind(receipt_ref.as_deref())
    .bind(donor_name.as_deref())
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "id": id }))),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "campanha entry insert");
            storage_error()
        }
    }
}

// ---------------------------------------------------------------------------
// DELETE /me/campanha/lancamentos/{id} — revoga (histórico permanece)
// ---------------------------------------------------------------------------

async fn revoke_entry(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    let res = sqlx::query(
        r"UPDATE campaign_finance_entry
             SET revoked_at = now()
           WHERE id = $1 AND citizen_id = $2 AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(citizen)
    .execute(&state.db)
    .await;
    match res {
        Ok(r) if r.rows_affected() > 0 => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "ok": true }))),
        )
            .into_response(),
        Ok(_) => fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "Lançamento não encontrado (ou já revogado).",
        ),
        Err(err) => {
            tracing::error!(?err, "campanha entry revoke");
            storage_error()
        }
    }
}

// ---------------------------------------------------------------------------
// PUT /me/campanha/config — upsert
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SaveConfigBody {
    #[serde(default)]
    meta_centavos: Option<i64>,
    #[serde(default)]
    bank_account: Option<String>,
    #[serde(default)]
    crowdfunding_url: Option<String>,
    #[serde(default)]
    is_published: bool,
}

async fn save_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SaveConfigBody>,
) -> Response {
    let Some(citizen) = caller_citizen(&headers) else {
        return unauthorized();
    };
    match is_politico(&state.db, citizen).await {
        Ok(true) => {}
        Ok(false) => return not_politico(),
        Err(err) => {
            tracing::error!(?err, "campanha gate check");
            return storage_error();
        }
    }
    if let Some(meta) = body.meta_centavos {
        if meta <= 0 || meta > MAX_VALOR_CENTAVOS {
            return fail(
                StatusCode::BAD_REQUEST,
                "invalid_meta",
                "Meta deve ser positiva (em centavos) e dentro do teto.",
            );
        }
    }
    let Ok(bank_account) = trimmed_opt(body.bank_account, MAX_BANK) else {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_bank",
            "Conta de campanha tem limite de 200 caracteres.",
        );
    };
    let Ok(crowdfunding_url) = trimmed_opt(body.crowdfunding_url, MAX_URL) else {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_url",
            "Link de financiamento coletivo tem limite de 300 caracteres.",
        );
    };
    if let Some(url) = crowdfunding_url.as_deref() {
        if !url.starts_with("https://") {
            return fail(
                StatusCode::BAD_REQUEST,
                "invalid_url",
                "O link de financiamento coletivo precisa ser https://.",
            );
        }
    }
    let res = sqlx::query(
        r"INSERT INTO campaign_fundraising_config
              (citizen_id, meta_centavos, bank_account, crowdfunding_url, is_published)
          VALUES ($1, $2, $3, $4, $5)
          ON CONFLICT (citizen_id)
          DO UPDATE SET meta_centavos    = EXCLUDED.meta_centavos,
                        bank_account     = EXCLUDED.bank_account,
                        crowdfunding_url = EXCLUDED.crowdfunding_url,
                        is_published     = EXCLUDED.is_published,
                        updated_at       = now()",
    )
    .bind(citizen)
    .bind(body.meta_centavos)
    .bind(bank_account.as_deref())
    .bind(crowdfunding_url.as_deref())
    .bind(body.is_published)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "ok": true }))),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(?err, "campanha config upsert");
            storage_error()
        }
    }
}

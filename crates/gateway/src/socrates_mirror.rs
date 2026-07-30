//! # SOCRATES espelha Ideias Legislativas do e-Cidadania (migrations 0670/0671).
//!
//! No portal e-Cidadania do Senado o cidadão SÓ pode apoiar uma Ideia
//! Legislativa — não existe votar contra nem argumentar. Aqui o debate é
//! completo (favor × contra + afirmação-ponte), então o SOCRATES (cidadão-bot
//! institucional, UUID fixo em 0670) espelha a ideia como tópico do fórum
//! `senado` pra abrir o debate que o portal não permite.
//!
//! Dois caminhos alimentam o mesmo espelho:
//!
//! 1. **Curadoria admin** (MVP, 0670): o admin cola a URL/ID → o gateway busca
//!    o título server-rendered (`<title>` / `og:description`; a descrição longa
//!    é SPA e NÃO vem no HTML estático) → cria o tópico assinado pelo bot.
//! 2. **Sweep automático** (v2, 0671): [`sweep_once`] lê a API pública JSON
//!    `restcolecaomaisideia` (as ideias EM ALTA, já com título e contador de
//!    apoios — nenhum fetch por ideia é necessário) e complementa com os ids
//!    linkados na página `principalideia`. As NOVAS viram tópico; as já
//!    espelhadas têm o contador de apoios re-sincronizado. Cada rodada vira uma
//!    linha em `socrates_sweep_run`.
//!
//! A coleção devolve ~5 itens e não pagina: o sweep é ACUMULATIVO por natureza
//! (roda a cada 6 h no worker e vai juntando o que o Senado promove ao topo),
//! e o teto `SOCRATES_SWEEP_MAX` impede que uma rodada anômala inunde o fórum.
//!
//! - `POST /admin/socrates/mirror`  — `{url_or_id}` → espelha (gate owner/admin).
//! - `GET  /admin/socrates/mirrors` — lista os espelhos existentes.
//! - `POST /admin/socrates/sweep`   — dispara uma rodada agora (gate owner/admin).
//! - `GET  /admin/socrates/runs`    — log das últimas rodadas.
//!
//! Autor-bot: o `create_topic` HTTP dos fóruns exige caller verificado; aqui o
//! gateway insere via `dsoc_forums::queries::insert_topic` DIRETO (mesma query
//! do service, contadores default idênticos), dentro de uma transação junto com
//! a linha `socrates_mirror` — atômico e sem passar pelo gate de sessão.
//!
//! Nenhum host vem de input: as três URLs do Senado são constantes deste
//! módulo, montadas a partir de um id NUMÉRICO validado — sem SSRF possível.

use axum::extract::{Json, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, Utc};
use dsoc_api_contract::{ApiError, ApiResponse};
use dsoc_app::AppState;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

/// Organização única da instância (mesma convenção do gateway single-org).
const DEFAULT_ORG_UUID: Uuid = uuid::uuid!("11111111-1111-1111-1111-111111111111");
/// O cidadão-bot SOCRATES (semeado com UUID fixo na migration 0670).
const SOCRATES_CITIZEN_ID: Uuid = uuid::uuid!("50c7a7e5-0000-4000-8000-000000000001");
/// Fórum destino dos espelhos (raiz institucional semeada por seed-forums.sql).
const SENADO_FORUM_PATH: &str = "senado";
/// User-Agent honesto no fetch ao portal do Senado.
const FETCH_USER_AGENT: &str = "democracia.social.br SOCRATES (contato: /contato)";
/// Timeout do fetch (o portal é lento em horário de pico; 10 s cobre).
const FETCH_TIMEOUT_SECS: u64 = 10;
/// Teto de dígitos do id numérico da ideia (hoje são 6; 12 dá folga de décadas).
const MAX_IDEIA_ID_DIGITS: usize = 12;
/// Teto da listagem de espelhos (painel admin; sem paginação no MVP).
const LIST_LIMIT: i64 = 200;
/// Teto da listagem de rodadas do sweep (o painel só mostra o histórico recente).
const RUNS_LIMIT: i64 = 50;
/// API pública JSON (sem auth) com as Ideias Legislativas EM ALTA. Devolve ~5
/// itens e NÃO pagina — por isso o sweep é incremental/acumulativo.
const COLLECTION_URL: &str = "https://www12.senado.leg.br/ecidadania/restcolecaomaisideia";
/// Página HTML de entrada das Ideias Legislativas; complementa a coleção com
/// ids que ainda não subiram pro topo (só temos o id, o título vem do fetch).
const PRINCIPAL_URL: &str = "https://www12.senado.leg.br/ecidadania/principalideia";
/// Marcador dos links de ideia dentro do HTML de `principalideia`.
const IDEIA_LINK_MARKER: &str = "visualizacaoideia?id=";
/// Teto default de espelhos NOVOS por rodada (env `SOCRATES_SWEEP_MAX`). Uma
/// rodada que estoure o teto deixa o resto pra próxima — o fórum nunca inunda.
const DEFAULT_SWEEP_MAX: usize = 10;
/// `socrates_mirror.origin` de um espelho colado por admin.
const ORIGIN_MANUAL: &str = "manual";
/// `socrates_mirror.origin` de um espelho descoberto pelo sweep.
const ORIGIN_SWEEP: &str = "sweep";
/// Teto do texto de erro consolidado gravado em `socrates_sweep_run.error`.
const RUN_ERROR_MAX_CHARS: usize = 500;
/// Prefixo server-rendered do `<title>` da página da ideia.
const TITLE_PREFIX: &str = "Ideia Legislativa - ";
/// Separador do sufixo institucional do `<title>` (":: Portal e-Cidadania …").
const TITLE_SUFFIX_SEP: &str = " :: ";
/// Prefixo do `og:description` server-rendered.
const OG_PREFIX: &str = "Apoie essa Ideia Legislativa:";

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/admin/socrates/mirror", post(mirror))
        .route("/admin/socrates/mirrors", get(list_mirrors))
        .route("/admin/socrates/sweep", post(run_sweep))
        .route("/admin/socrates/runs", get(list_runs))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Gate owner/admin (mesmo padrão de profile_nudge/admin_forums)
// ---------------------------------------------------------------------------

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

/// Gate owner/admin. Retorna Err(resposta pronta) quando não passa.
async fn require_admin(db: &PgPool, headers: &HeaderMap) -> Result<Uuid, Response> {
    let Some(citizen) = caller_citizen(headers) else {
        return Err(fail(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Autenticação necessária.",
        ));
    };
    let is_admin: bool = sqlx::query_scalar(
        r"SELECT EXISTS(
            SELECT 1 FROM admin_role_binding
             WHERE citizen_id = $1 AND role IN ('owner','admin'))",
    )
    .bind(citizen)
    .fetch_one(db)
    .await
    .unwrap_or(false);
    if is_admin {
        Ok(citizen)
    } else {
        Err(fail(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Requer administrador.",
        ))
    }
}

// ---------------------------------------------------------------------------
// Parse do input do admin + do HTML server-rendered do e-Cidadania
// ---------------------------------------------------------------------------

/// Extrai o id NUMÉRICO da ideia: aceita o id puro ("165188") ou uma URL do
/// e-Cidadania contendo `id=NNNNNN` na query string. `None` = entrada inválida.
///
/// A URL colada NUNCA é buscada — o fetch usa sempre a URL canônica montada a
/// partir do id extraído (sem SSRF por URL arbitrária).
fn parse_ideia_id(input: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    // Id puro: só dígitos.
    if input.chars().all(|c| c.is_ascii_digit()) {
        return (input.len() <= MAX_IDEIA_ID_DIGITS).then(|| input.to_owned());
    }
    // URL: primeiro parâmetro `id=` da query string (`?id=` ou `&id=`).
    let query = input.split_once('?').map(|(_, q)| q)?;
    for pair in query.split('&') {
        if let Some(v) = pair.strip_prefix("id=") {
            // Corta em qualquer sujeira pós-numérica (ex.: fragmento "#apoios").
            let digits: String = v.chars().take_while(char::is_ascii_digit).collect();
            if !digits.is_empty() && digits.len() <= MAX_IDEIA_ID_DIGITS {
                return Some(digits);
            }
        }
    }
    None
}

/// URL canônica da ideia no portal do Senado.
fn canonical_url(ideia_id: &str) -> String {
    format!("https://www12.senado.leg.br/ecidadania/visualizacaoideia?id={ideia_id}")
}

/// Decodifica as entidades HTML que aparecem em títulos reais do portal.
/// (`&amp;` por último pra não re-expandir `&amp;quot;`.)
fn decode_entities(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&#039;", "'")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

/// Limpa o conteúdo do `<title>`: remove o prefixo "Ideia Legislativa - " e o
/// sufixo institucional a partir do ÚLTIMO " :: " (":: Portal e-Cidadania -
/// Senado Federal"). `None` quando sobra vazio.
fn clean_title_tag(raw: &str) -> Option<String> {
    let decoded = decode_entities(raw);
    let mut t = decoded.trim().to_owned();
    if let Some(rest) = t.strip_prefix(TITLE_PREFIX) {
        t = rest.to_owned();
    }
    if let Some(idx) = t.rfind(TITLE_SUFFIX_SEP) {
        t.truncate(idx);
    }
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

/// Limpa o `og:description` ('Apoie essa Ideia Legislativa: "<TÍTULO>"'):
/// remove o prefixo e as aspas envolventes. `None` quando sobra vazio.
fn clean_og_description(raw: &str) -> Option<String> {
    let decoded = decode_entities(raw);
    let mut t = decoded.trim();
    if let Some(rest) = t.strip_prefix(OG_PREFIX) {
        t = rest.trim();
    }
    t = t.strip_prefix('"').unwrap_or(t);
    t = t.strip_suffix('"').unwrap_or(t);
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

/// Conteúdo bruto do `<title>…</title>` (case-insensitive no nome da tag).
fn title_tag(html: &str) -> Option<&str> {
    let lower = html.to_lowercase();
    let open = lower.find("<title")?;
    let open_end = open + lower[open..].find('>')?;
    let close = open_end + lower[open_end..].find("</title")?;
    html.get(open_end + 1..close)
}

/// Conteúdo do atributo `content` da meta `og:description`, com `content`
/// aceito ANTES ou DEPOIS de `property` (a ordem varia entre renderizações).
fn og_description(html: &str) -> Option<&str> {
    let marker = html.find("og:description")?;
    // Delimita a tag `<meta …>` que contém o marcador.
    let tag_start = html[..marker].rfind('<')?;
    let tag_end = marker + html[marker..].find('>')?;
    let tag = html.get(tag_start..tag_end)?;
    let content_at = tag.find("content=")?;
    let after = &tag[content_at + "content=".len()..];
    let quote = after.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let inner = &after[1..];
    let end = inner.find(quote)?;
    inner.get(..end)
}

/// Título da ideia a partir do HTML server-rendered: `<title>` primeiro,
/// `og:description` como fallback. `None` = página fora do formato conhecido.
fn extract_title(html: &str) -> Option<String> {
    if let Some(t) = title_tag(html).and_then(clean_title_tag) {
        // O <title> genérico do portal (sem " - <TÍTULO>") vira só o nome do
        // site após a limpeza; rejeita e tenta o og:description.
        if t != "Portal e-Cidadania - Senado Federal" {
            return Some(t);
        }
    }
    og_description(html).and_then(clean_og_description)
}

/// Corpo (markdown) do tópico espelhado — o texto integral não vem no HTML
/// estático (SPA), então o corpo aponta pro link canônico com atribuição.
///
/// `apoiamentos` é o contador COMO O SENADO FORMATA ("20.771"): quando o sweep
/// o traz na coleção, ele entra como contexto ("quantos já apoiaram lá") logo
/// abaixo do título. No caminho admin (sem coleção) o bloco simplesmente some.
fn topic_body(title: &str, url: &str, apoiamentos: Option<&str>) -> String {
    let apoios_line = match apoiamentos {
        Some(a) => format!("📊 **{a} apoios** no e-Cidadania até agora.\n\n"),
        None => String::new(),
    };
    format!(
        "**Ideia Legislativa espelhada do e-Cidadania (Senado Federal)** 🏛️\n\n\
         > {title}\n\n\
         {apoios_line}\
         No portal do Senado, cidadãos só podem **apoiar** esta ideia — não há como argumentar \
         contra, ponderar ou debater. Aqui no DemocraciaBR o debate é completo: **argumente a \
         favor ou contra e vote**.\n\n\
         📌 Ideia original: {url}\n\
         (Se ela atingir 20.000 apoios no e-Cidadania, vira sugestão legislativa formal no \
         Senado.)\n\n\
         — *SOCRATES, agente cívico da plataforma. Conteúdo público do Senado Federal, \
         espelhado com atribuição.*"
    )
}

// ---------------------------------------------------------------------------
// Descoberta automática: coleção JSON + ids linkados no HTML
// ---------------------------------------------------------------------------

/// Uma ideia candidata a espelho. O `titulo`/`apoiamentos` só existem quando a
/// candidata veio da coleção JSON; vindo do HTML (ou do painel admin) só há id,
/// e o título é buscado na página canônica.
#[derive(Debug, Clone, PartialEq, Eq)]
struct IdeaCandidate {
    ideia_id: String,
    titulo: Option<String>,
    /// Contador de apoios COMO O SENADO FORMATA ("20.771").
    apoiamentos: Option<String>,
    porcentagem_favor: Option<i32>,
}

impl IdeaCandidate {
    /// Candidata sem metadados — o caminho do painel admin e do HTML.
    fn bare(ideia_id: String) -> Self {
        Self {
            ideia_id,
            titulo: None,
            apoiamentos: None,
            porcentagem_favor: None,
        }
    }

    /// Há dado de apoios pra gravar/re-sincronizar?
    fn has_apoios(&self) -> bool {
        self.apoiamentos.is_some() || self.porcentagem_favor.is_some()
    }
}

/// Texto de um campo JSON que o portal ora manda como string ("20.771"), ora
/// poderia mandar como número — os dois viram texto, nada é reinterpretado.
fn json_text(value: &serde_json::Value) -> Option<String> {
    let text = match value {
        serde_json::Value::String(s) => s.trim().to_owned(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => return None,
    };
    (!text.is_empty()).then_some(text)
}

/// Parseia o array de `restcolecaomaisideia`. Item fora do formato esperado é
/// DESCARTADO em silêncio (a rodada segue com o resto) — o portal é de
/// terceiros e uma mudança de shape não pode derrubar o sweep.
fn parse_collection(json: &str) -> Vec<IdeaCandidate> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(items) = value.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            // O id é numérico na prática; `parse_ideia_id` valida os dois casos
            // (número ou string) contra o mesmo teto de dígitos do painel.
            let ideia_id = parse_ideia_id(&json_text(item.get("id")?)?)?;
            Some(IdeaCandidate {
                ideia_id,
                titulo: item
                    .get("titulo")
                    .and_then(json_text)
                    .map(|t| decode_entities(&t).trim().to_owned())
                    .filter(|t| !t.is_empty()),
                apoiamentos: item.get("apoiamentos").and_then(json_text),
                porcentagem_favor: item
                    .get("porcentagemFavor")
                    .and_then(serde_json::Value::as_i64)
                    .and_then(|n| i32::try_from(n).ok()),
            })
        })
        .collect()
}

/// Ids das ideias linkadas no HTML de `principalideia` (`visualizacaoideia?id=NNNNNN`),
/// na ordem de aparição e sem repetição.
fn extract_ideia_ids(html: &str) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let mut rest = html;
    while let Some(pos) = rest.find(IDEIA_LINK_MARKER) {
        let after = &rest[pos + IDEIA_LINK_MARKER.len()..];
        let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() && digits.len() <= MAX_IDEIA_ID_DIGITS && !ids.contains(&digits) {
            ids.push(digits);
        }
        rest = after;
    }
    ids
}

/// Junta as duas fontes: a coleção manda (traz título + apoios) e o HTML só
/// acrescenta os ids que ela não cobriu.
fn merge_candidates(collection: Vec<IdeaCandidate>, html_ids: Vec<String>) -> Vec<IdeaCandidate> {
    let mut merged = collection;
    for id in html_ids {
        if !merged.iter().any(|c| c.ideia_id == id) {
            merged.push(IdeaCandidate::bare(id));
        }
    }
    merged
}

/// Trunca o título ao teto dos fóruns preservando caracteres inteiros.
fn clamp_title(title: &str) -> String {
    let max = dsoc_forums::domain::MAX_TITLE_LEN;
    if title.chars().count() <= max {
        return title.to_owned();
    }
    let mut t: String = title.chars().take(max - 1).collect();
    t.push('…');
    t
}

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct MirrorRequest {
    /// URL da ideia no e-Cidadania OU o id numérico puro.
    url_or_id: String,
}

#[derive(Debug, Serialize)]
struct MirrorCreated {
    topic_id: Uuid,
    /// Caminho navegável do tópico no front (`/f/topico/<id>`).
    path: String,
}

#[derive(Debug, Serialize)]
struct MirrorEntry {
    ideia_id: String,
    source_url: String,
    topic_id: Uuid,
    topic_title: String,
    path: String,
    created_at: DateTime<Utc>,
    /// Contador de apoios no e-Cidadania (formatação do Senado); `null` até o
    /// primeiro sweep que vir a ideia na coleção.
    apoiamentos: Option<String>,
    porcentagem_favor: Option<i32>,
    apoios_updated_at: Option<DateTime<Utc>>,
    /// `manual` (admin colou) ou `sweep` (descoberto pelo worker).
    origin: String,
}

/// Resultado de uma rodada de sweep — o que o painel mostra e o que vai pro log.
#[derive(Debug, Default, Clone, Serialize)]
pub struct SweepStats {
    /// Ideias distintas vistas nas duas fontes.
    pub found: usize,
    /// Ideias que viraram tópico novo nesta rodada.
    pub mirrored: usize,
    /// Ideias ignoradas: já espelhadas, teto da rodada estourado, ou falha.
    pub skipped: usize,
    /// Espelhos existentes que tiveram o contador de apoios re-sincronizado.
    pub updated: usize,
    /// Erros não-fatais da rodada (fetch de uma fonte, espelho de uma ideia).
    pub errors: Vec<String>,
}

impl SweepStats {
    /// Erros consolidados pro `socrates_sweep_run.error` (`None` = rodada limpa).
    fn error_text(&self) -> Option<String> {
        if self.errors.is_empty() {
            return None;
        }
        let mut joined = self.errors.join(" | ");
        if joined.chars().count() > RUN_ERROR_MAX_CHARS {
            joined = joined.chars().take(RUN_ERROR_MAX_CHARS).collect();
        }
        Some(joined)
    }
}

#[derive(Debug, Serialize)]
struct SweepRunEntry {
    id: Uuid,
    started_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
    found: i32,
    mirrored: i32,
    skipped: i32,
    error: Option<String>,
}

/// Por que uma ideia NÃO virou tópico. O caminho admin traduz cada variante num
/// status HTTP; o sweep só conta e loga.
#[derive(Debug)]
enum MirrorError {
    /// Já existe espelho — o `Uuid` é o tópico existente (o painel linka nele).
    AlreadyMirrored(Uuid),
    /// O portal do Senado não respondeu / respondeu fora do formato.
    Upstream(String),
    /// O fórum `senado` não existe nesta instalação.
    ForumMissing,
    /// Falha de banco (já logada com contexto no ponto de origem).
    Storage,
}

/// 409 com o tópico existente no `data` (o painel linka direto) — o envelope
/// carrega erro E payload aqui de propósito: o conflito é INFORMATIVO.
fn already_mirrored(ideia_id: &str, topic_id: Uuid) -> Response {
    let body = ApiResponse {
        success: false,
        data: Some(MirrorCreated {
            topic_id,
            path: format!("/f/topico/{topic_id}"),
        }),
        error: Some(ApiError {
            code: "already_mirrored".to_owned(),
            message: format!("A ideia {ideia_id} já foi espelhada — o tópico já existe."),
        }),
        meta: None,
    };
    (StatusCode::CONFLICT, Json(body)).into_response()
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Espelha UMA ideia: dedup → título → tópico + linha `socrates_mirror` na
/// MESMA transação. É o núcleo compartilhado pelos dois caminhos (painel admin
/// e sweep); devolve o id do tópico criado.
///
/// O título só custa um fetch quando a candidata não trouxe um (caso do painel
/// e dos ids vindos do HTML) — pela coleção JSON ele já vem pronto.
async fn mirror_candidate(
    state: &AppState,
    candidate: &IdeaCandidate,
    origin: &str,
) -> Result<Uuid, MirrorError> {
    let ideia_id = &candidate.ideia_id;

    // Dedup ANTES do fetch: ideia já espelhada nunca dispara rede.
    match sqlx::query_scalar::<_, Uuid>("SELECT topic_id FROM socrates_mirror WHERE ideia_id = $1")
        .bind(ideia_id)
        .fetch_optional(&state.db)
        .await
    {
        Ok(Some(existing)) => return Err(MirrorError::AlreadyMirrored(existing)),
        Ok(None) => {}
        Err(err) => {
            tracing::error!(?err, "socrates: dedup lookup falhou");
            return Err(MirrorError::Storage);
        }
    }

    let source_url = canonical_url(ideia_id);
    let title = match candidate.titulo.clone() {
        Some(title) => title,
        None => {
            let html = fetch_text(&source_url)
                .await
                .map_err(MirrorError::Upstream)?;
            extract_title(&html).ok_or_else(|| {
                MirrorError::Upstream(
                    "A página do e-Cidadania respondeu, mas não foi possível extrair o título da \
                     ideia (formato inesperado — a ideia existe?)."
                        .to_owned(),
                )
            })?
        }
    };
    let title = clamp_title(&title);
    let body = topic_body(&title, &source_url, candidate.apoiamentos.as_deref());
    let new = dsoc_forums::domain::NewTopic::validate(&title, &body).map_err(|err| {
        tracing::error!(?err, "socrates: título/corpo fora dos limites dos fóruns");
        MirrorError::Upstream(
            "O título extraído do e-Cidadania não passou na validação dos fóruns.".to_owned(),
        )
    })?;

    // Fórum destino: a raiz `senado` (institucional, semeada — nunca é
    // materializável on-demand como as seções territoriais).
    let forum_id: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM forum WHERE org_id = $1 AND full_path = $2")
            .bind(DEFAULT_ORG_UUID)
            .bind(SENADO_FORUM_PATH)
            .fetch_optional(&state.db)
            .await
            .map_err(|err| {
                tracing::error!(?err, "socrates: lookup do fórum senado falhou");
                MirrorError::Storage
            })?;
    let Some(forum_id) = forum_id else {
        return Err(MirrorError::ForumMissing);
    };

    // Tópico + espelho na MESMA transação: ou os dois existem, ou nenhum.
    // `insert_topic` é a MESMA query do ForumService::create_topic (contadores
    // default idênticos); inserir direto evita o gate de sessão/verificação do
    // caminho HTTP — o autor é o cidadão-bot, sem credencial.
    let now = state.clock.now();
    let mut tx = state.db.begin().await.map_err(|err| {
        tracing::error!(?err, "socrates: begin falhou");
        MirrorError::Storage
    })?;
    let topic = dsoc_forums::queries::insert_topic(
        &mut *tx,
        Uuid::now_v7(),
        forum_id,
        SOCRATES_CITIZEN_ID,
        &new.title,
        &new.body,
        now,
    )
    .await
    .map_err(|err| {
        tracing::error!(?err, "socrates: insert_topic falhou");
        MirrorError::Storage
    })?;
    let inserted = sqlx::query(
        "INSERT INTO socrates_mirror
             (id, ideia_id, source_url, topic_id, created_at,
              apoiamentos, porcentagem_favor, apoios_updated_at, origin)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         ON CONFLICT (ideia_id) DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(ideia_id)
    .bind(&source_url)
    .bind(topic.id)
    .bind(now)
    .bind(candidate.apoiamentos.as_deref())
    .bind(candidate.porcentagem_favor)
    .bind(candidate.has_apoios().then_some(now))
    .bind(origin)
    .execute(&mut *tx)
    .await;
    match inserted {
        // Corrida: outro caminho espelhou entre o dedup e o commit — aborta o
        // tópico (rollback implícito no drop da tx) e reporta o conflito.
        Ok(r) if r.rows_affected() == 0 => {
            drop(tx);
            return match sqlx::query_scalar::<_, Uuid>(
                "SELECT topic_id FROM socrates_mirror WHERE ideia_id = $1",
            )
            .bind(ideia_id)
            .fetch_optional(&state.db)
            .await
            {
                Ok(Some(existing)) => Err(MirrorError::AlreadyMirrored(existing)),
                _ => Err(MirrorError::Storage),
            };
        }
        Ok(_) => {}
        Err(err) => {
            tracing::error!(?err, "socrates: insert do espelho falhou");
            return Err(MirrorError::Storage);
        }
    }
    tx.commit().await.map_err(|err| {
        tracing::error!(?err, "socrates: commit falhou");
        MirrorError::Storage
    })?;
    Ok(topic.id)
}

/// `POST /admin/socrates/mirror` — espelha uma Ideia Legislativa como tópico
/// do fórum `senado`, assinado pelo cidadão-bot SOCRATES.
async fn mirror(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<MirrorRequest>,
) -> Response {
    if let Err(resp) = require_admin(&state.db, &headers).await {
        return resp;
    }
    let Some(ideia_id) = parse_ideia_id(&req.url_or_id) else {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "Informe a URL da ideia no e-Cidadania (…visualizacaoideia?id=NNNNNN) ou o id numérico.",
        );
    };
    let candidate = IdeaCandidate::bare(ideia_id.clone());
    match mirror_candidate(&state, &candidate, ORIGIN_MANUAL).await {
        Ok(topic_id) => {
            let dto = MirrorCreated {
                topic_id,
                path: format!("/f/topico/{topic_id}"),
            };
            (StatusCode::CREATED, Json(ApiResponse::ok(dto))).into_response()
        }
        Err(MirrorError::AlreadyMirrored(existing)) => already_mirrored(&ideia_id, existing),
        Err(MirrorError::Upstream(msg)) => fail(StatusCode::BAD_GATEWAY, "upstream_error", &msg),
        Err(MirrorError::ForumMissing) => fail(
            StatusCode::INTERNAL_SERVER_ERROR,
            "forum_missing",
            "O fórum 'senado' não existe nesta instalação (rode scripts/seed-forums.sql).",
        ),
        Err(MirrorError::Storage) => storage_error(),
    }
}

/// `GET /admin/socrates/mirrors` — os espelhos existentes, mais recentes primeiro.
async fn list_mirrors(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_admin(&state.db, &headers).await {
        return resp;
    }
    type Row = (
        String,
        String,
        Uuid,
        String,
        DateTime<Utc>,
        Option<String>,
        Option<i32>,
        Option<DateTime<Utc>>,
        String,
    );
    let rows: Result<Vec<Row>, sqlx::Error> = sqlx::query_as(
        "SELECT m.ideia_id, m.source_url, m.topic_id, t.title, m.created_at,
                m.apoiamentos, m.porcentagem_favor, m.apoios_updated_at, m.origin
           FROM socrates_mirror m
           JOIN forum_topic t ON t.id = m.topic_id
          ORDER BY m.created_at DESC
          LIMIT $1",
    )
    .bind(LIST_LIMIT)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => {
            let dtos: Vec<MirrorEntry> = rows
                .into_iter()
                .map(
                    |(
                        ideia_id,
                        source_url,
                        topic_id,
                        topic_title,
                        created_at,
                        apoiamentos,
                        porcentagem_favor,
                        apoios_updated_at,
                        origin,
                    )| MirrorEntry {
                        ideia_id,
                        source_url,
                        topic_id,
                        topic_title,
                        path: format!("/f/topico/{topic_id}"),
                        created_at,
                        apoiamentos,
                        porcentagem_favor,
                        apoios_updated_at,
                        origin,
                    },
                )
                .collect();
            (StatusCode::OK, Json(ApiResponse::ok(dtos))).into_response()
        }
        Err(err) => {
            tracing::error!(?err, "socrates: listagem falhou");
            storage_error()
        }
    }
}

/// `POST /admin/socrates/sweep` — dispara UMA rodada agora e devolve o
/// resultado. Síncrono de propósito: o admin quer ver o que aconteceu.
async fn run_sweep(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_admin(&state.db, &headers).await {
        return resp;
    }
    match sweep_once(&state).await {
        Ok(stats) => (StatusCode::OK, Json(ApiResponse::ok(stats))).into_response(),
        Err(msg) => fail(StatusCode::BAD_GATEWAY, "upstream_error", &msg),
    }
}

/// `GET /admin/socrates/runs` — as últimas rodadas do sweep, recentes primeiro.
async fn list_runs(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_admin(&state.db, &headers).await {
        return resp;
    }
    type Row = (
        Uuid,
        DateTime<Utc>,
        Option<DateTime<Utc>>,
        i32,
        i32,
        i32,
        Option<String>,
    );
    let rows: Result<Vec<Row>, sqlx::Error> = sqlx::query_as(
        "SELECT id, started_at, finished_at, found, mirrored, skipped, error
           FROM socrates_sweep_run
          ORDER BY started_at DESC
          LIMIT $1",
    )
    .bind(RUNS_LIMIT)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => {
            let dtos: Vec<SweepRunEntry> = rows
                .into_iter()
                .map(
                    |(id, started_at, finished_at, found, mirrored, skipped, error)| {
                        SweepRunEntry {
                            id,
                            started_at,
                            finished_at,
                            found,
                            mirrored,
                            skipped,
                            error,
                        }
                    },
                )
                .collect();
            (StatusCode::OK, Json(ApiResponse::ok(dtos))).into_response()
        }
        Err(err) => {
            tracing::error!(?err, "socrates: listagem de rodadas falhou");
            storage_error()
        }
    }
}

// ---------------------------------------------------------------------------
// Sweep automático (v2)
// ---------------------------------------------------------------------------

/// Teto de espelhos NOVOS por rodada (`SOCRATES_SWEEP_MAX`, default 10).
fn sweep_max() -> usize {
    parse_sweep_max(std::env::var("SOCRATES_SWEEP_MAX").ok().as_deref())
}

/// Lê o teto da rodada. Valor ausente, ilegível ou zero cai no default — um
/// teto 0 desligaria o sweep silenciosamente, o que nunca é a intenção.
fn parse_sweep_max(raw: Option<&str>) -> usize {
    raw.and_then(|v| v.trim().parse().ok())
        .filter(|&v: &usize| v > 0)
        .unwrap_or(DEFAULT_SWEEP_MAX)
}

/// UMA rodada do sweep: descobre as ideias em alta, espelha as novas (até o
/// teto) e re-sincroniza o contador de apoios das já espelhadas. Grava a rodada
/// em `socrates_sweep_run` — inclusive quando ela falha, pra que "o sweep está
/// quebrado" seja visível no painel em vez de silencioso.
///
/// `Err` só quando NENHUMA fonte respondeu: aí não há o que espelhar e a
/// diferença entre "o Senado está fora" e "não há ideia nova" importa.
pub async fn sweep_once(state: &AppState) -> Result<SweepStats, String> {
    let run_id = Uuid::now_v7();
    let started_at = state.clock.now();
    // A linha nasce ANTES da rede: uma rodada travada aparece no painel com
    // `finished_at` nulo em vez de sumir.
    if let Err(err) = sqlx::query(
        "INSERT INTO socrates_sweep_run (id, started_at, found, mirrored, skipped)
         VALUES ($1, $2, 0, 0, 0)",
    )
    .bind(run_id)
    .bind(started_at)
    .execute(&state.db)
    .await
    {
        tracing::error!(?err, "socrates sweep: abertura da rodada falhou");
    }

    let outcome = sweep_inner(state, sweep_max()).await;
    let (stats, error_text) = match &outcome {
        Ok(stats) => (stats.clone(), stats.error_text()),
        Err(msg) => (SweepStats::default(), Some(msg.clone())),
    };
    if let Err(err) = sqlx::query(
        "UPDATE socrates_sweep_run
            SET finished_at = $2, found = $3, mirrored = $4, skipped = $5, error = $6
          WHERE id = $1",
    )
    .bind(run_id)
    .bind(state.clock.now())
    .bind(i32::try_from(stats.found).unwrap_or(i32::MAX))
    .bind(i32::try_from(stats.mirrored).unwrap_or(i32::MAX))
    .bind(i32::try_from(stats.skipped).unwrap_or(i32::MAX))
    .bind(error_text)
    .execute(&state.db)
    .await
    {
        tracing::error!(?err, "socrates sweep: fechamento da rodada falhou");
    }
    outcome
}

/// O trabalho da rodada, sem o log (que [`sweep_once`] gerencia).
async fn sweep_inner(state: &AppState, max_new: usize) -> Result<SweepStats, String> {
    let mut stats = SweepStats::default();

    // As duas fontes são independentes: a queda de uma não cancela a outra.
    let collection = match fetch_text(COLLECTION_URL).await {
        Ok(json) => parse_collection(&json),
        Err(msg) => {
            stats.errors.push(format!("coleção: {msg}"));
            Vec::new()
        }
    };
    let html_ids = match fetch_text(PRINCIPAL_URL).await {
        Ok(html) => extract_ideia_ids(&html),
        Err(msg) => {
            stats.errors.push(format!("principalideia: {msg}"));
            Vec::new()
        }
    };
    if collection.is_empty() && html_ids.is_empty() {
        return Err(if stats.errors.is_empty() {
            "Nenhuma Ideia Legislativa encontrada nas fontes do e-Cidadania.".to_owned()
        } else {
            stats.errors.join(" | ")
        });
    }

    let candidates = merge_candidates(collection, html_ids);
    stats.found = candidates.len();

    // Uma consulta pra saber quem já está espelhado — evita N lookups.
    let ids: Vec<String> = candidates.iter().map(|c| c.ideia_id.clone()).collect();
    let existing: Vec<String> =
        sqlx::query_scalar("SELECT ideia_id FROM socrates_mirror WHERE ideia_id = ANY($1)")
            .bind(&ids)
            .fetch_all(&state.db)
            .await
            .map_err(|err| {
                tracing::error!(?err, "socrates sweep: dedup em lote falhou");
                "Erro ao consultar os espelhos existentes.".to_owned()
            })?;

    let now = state.clock.now();
    for candidate in &candidates {
        if existing.contains(&candidate.ideia_id) {
            stats.skipped += 1;
            // Já espelhada: o que muda com o tempo é só o contador de apoios.
            if candidate.has_apoios() && refresh_apoios(state, candidate, now).await {
                stats.updated += 1;
            }
            continue;
        }
        if stats.mirrored >= max_new {
            // Teto da rodada: o resto entra na próxima (a cada 6 h).
            stats.skipped += 1;
            continue;
        }
        match mirror_candidate(state, candidate, ORIGIN_SWEEP).await {
            Ok(topic_id) => {
                stats.mirrored += 1;
                tracing::info!(
                    ideia_id = %candidate.ideia_id,
                    %topic_id,
                    "socrates sweep: ideia espelhada"
                );
            }
            // Corrida com o painel admin — não é erro, só não é nova.
            Err(MirrorError::AlreadyMirrored(_)) => stats.skipped += 1,
            Err(err) => {
                stats.skipped += 1;
                let msg = match err {
                    MirrorError::Upstream(msg) => msg,
                    MirrorError::ForumMissing => {
                        "o fórum 'senado' não existe nesta instalação".to_owned()
                    }
                    _ => "erro de armazenamento".to_owned(),
                };
                stats
                    .errors
                    .push(format!("ideia {}: {msg}", candidate.ideia_id));
            }
        }
    }
    Ok(stats)
}

/// Re-sincroniza `apoiamentos`/`porcentagem_favor`/`apoios_updated_at` de um
/// espelho existente. `false` = nada foi atualizado (falha logada, rodada segue).
async fn refresh_apoios(state: &AppState, candidate: &IdeaCandidate, now: DateTime<Utc>) -> bool {
    let updated = sqlx::query(
        "UPDATE socrates_mirror
            SET apoiamentos = $2, porcentagem_favor = $3, apoios_updated_at = $4
          WHERE ideia_id = $1",
    )
    .bind(&candidate.ideia_id)
    .bind(candidate.apoiamentos.as_deref())
    .bind(candidate.porcentagem_favor)
    .bind(now)
    .execute(&state.db)
    .await;
    match updated {
        Ok(r) => r.rows_affected() > 0,
        Err(err) => {
            tracing::warn!(
                ?err,
                ideia_id = %candidate.ideia_id,
                "socrates sweep: refresh de apoios falhou"
            );
            false
        }
    }
}

/// GET simples numa URL do portal do Senado (constante deste módulo). Serve
/// tanto a página da ideia (HTML) quanto a coleção (JSON) — o corpo é texto.
async fn fetch_text(url: &str) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
        .user_agent(FETCH_USER_AGENT)
        .build()
        .map_err(|_| "Não foi possível montar o cliente HTTP.".to_owned())?;
    let resp = client.get(url).send().await.map_err(|err| {
        tracing::warn!(?err, url, "socrates: fetch ao e-Cidadania falhou");
        "O portal e-Cidadania não respondeu (timeout/rede). Tente novamente.".to_owned()
    })?;
    if !resp.status().is_success() {
        return Err(format!(
            "O portal e-Cidadania respondeu com status {} — confira o id da ideia.",
            resp.status().as_u16()
        ));
    }
    resp.text()
        .await
        .map_err(|_| "Falha ao ler a resposta do e-Cidadania.".to_owned())
}

// ---------------------------------------------------------------------------
// Testes de unidade — parse do id e limpeza do título (fixtures inline; o
// Senado NUNCA é chamado em teste).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pure_numeric_id() {
        assert_eq!(parse_ideia_id("165188").as_deref(), Some("165188"));
        assert_eq!(parse_ideia_id("  42  ").as_deref(), Some("42"));
    }

    #[test]
    fn parses_id_from_ecidadania_url() {
        assert_eq!(
            parse_ideia_id("https://www12.senado.leg.br/ecidadania/visualizacaoideia?id=165188")
                .as_deref(),
            Some("165188")
        );
        // Parâmetros extras e fragmento não atrapalham.
        assert_eq!(
            parse_ideia_id(
                "https://www12.senado.leg.br/ecidadania/visualizacaoideia?utm=x&id=99#apoios"
            )
            .as_deref(),
            Some("99")
        );
    }

    #[test]
    fn rejects_invalid_input() {
        assert!(parse_ideia_id("").is_none());
        assert!(parse_ideia_id("abc").is_none());
        assert!(parse_ideia_id("https://example.com/sem-id").is_none());
        assert!(parse_ideia_id("https://x/?id=").is_none());
        assert!(parse_ideia_id("https://x/?id=abc").is_none());
        // Estouro do teto de dígitos.
        assert!(parse_ideia_id("1234567890123").is_none());
    }

    #[test]
    fn cleans_title_tag_format() {
        assert_eq!(
            clean_title_tag(
                "Ideia Legislativa - Fim da reeleição no Legislativo \
                 :: Portal e-Cidadania - Senado Federal"
            )
            .as_deref(),
            Some("Fim da reeleição no Legislativo")
        );
        // Sem prefixo/sufixo: passa como está.
        assert_eq!(
            clean_title_tag(" Só o título ").as_deref(),
            Some("Só o título")
        );
        assert!(clean_title_tag("   ").is_none());
    }

    #[test]
    fn cleans_og_description_format() {
        assert_eq!(
            clean_og_description("Apoie essa Ideia Legislativa: \"Tarifa zero nacional\"")
                .as_deref(),
            Some("Tarifa zero nacional")
        );
        assert_eq!(
            clean_og_description("Apoie essa Ideia Legislativa: &quot;Tarifa zero&quot;")
                .as_deref(),
            Some("Tarifa zero")
        );
        assert!(clean_og_description("Apoie essa Ideia Legislativa: \"\"").is_none());
    }

    #[test]
    fn extracts_title_from_title_tag_fixture() {
        let html = r#"<!doctype html><html><head>
            <title>Ideia Legislativa - Piso nacional da enfermagem valendo j&aacute; :: Portal e-Cidadania - Senado Federal</title>
            <meta property="og:description" content="Apoie essa Ideia Legislativa: &quot;Fallback n&atilde;o usado&quot;" />
            </head><body></body></html>"#;
        // Entidades fora da tabela mínima (&aacute;) passam intactas; o corte
        // prefixo/sufixo do formato real é o que está sob teste.
        assert_eq!(
            extract_title(html).as_deref(),
            Some("Piso nacional da enfermagem valendo j&aacute;")
        );
    }

    #[test]
    fn falls_back_to_og_description_when_title_is_generic() {
        let html = r#"<head><title>Portal e-Cidadania - Senado Federal</title>
            <meta content='Apoie essa Ideia Legislativa: "Voto distrital misto"' property="og:description">
            </head>"#;
        assert_eq!(extract_title(html).as_deref(), Some("Voto distrital misto"));
    }

    #[test]
    fn extract_title_none_on_unrecognized_html() {
        assert!(extract_title("<html><body>404</body></html>").is_none());
    }

    #[test]
    fn decodes_common_entities() {
        assert_eq!(
            decode_entities("A &amp; B &quot;C&quot; &#39;D&#39;"),
            "A & B \"C\" 'D'"
        );
    }

    #[test]
    fn clamps_long_titles_to_forum_limit() {
        let long = "x".repeat(dsoc_forums::domain::MAX_TITLE_LEN + 50);
        let clamped = clamp_title(&long);
        assert_eq!(clamped.chars().count(), dsoc_forums::domain::MAX_TITLE_LEN);
        assert!(clamped.ends_with('…'));
        assert_eq!(clamp_title("curto"), "curto");
    }

    #[test]
    fn canonical_url_pins_senado_host() {
        assert_eq!(
            canonical_url("165188"),
            "https://www12.senado.leg.br/ecidadania/visualizacaoideia?id=165188"
        );
    }

    #[test]
    fn topic_body_carries_attribution_and_link() {
        let b = topic_body("Título X", "https://exemplo/id=1", None);
        assert!(b.contains("> Título X"));
        assert!(b.contains("https://exemplo/id=1"));
        assert!(b.contains("SOCRATES"));
        assert!(b.contains("20.000 apoios"));
        // Sem contador do Senado, o bloco de apoios não aparece.
        assert!(!b.contains("📊"));
    }

    #[test]
    fn topic_body_shows_senado_support_count_when_known() {
        let b = topic_body("Gasolina pura", "https://exemplo/id=1", Some("20.771"));
        assert!(b.contains("📊 **20.771 apoios** no e-Cidadania"));
        // A mensagem-tese continua lá — o contador é contexto, não substituto.
        assert!(b.contains("argumente a favor ou contra"));
        assert!(b.contains("SOCRATES"));
    }

    // --- Sweep: parse das duas fontes públicas do e-Cidadania ---------------
    // Fixtures inline com o shape REAL observado ao vivo; o Senado nunca é
    // chamado em teste.

    /// Shape real de `GET /ecidadania/restcolecaomaisideia` (array, ~5 itens).
    const COLLECTION_FIXTURE: &str = r#"[
        {"count":"3.235","titulo":"Disponibilização de Gasolina Pura","id":227319,
         "porcentagemFavor":103,"apoiamentos":"20.771"},
        {"count":"1.002","titulo":"Piso nacional da enfermagem","id":165188,
         "porcentagemFavor":98,"apoiamentos":"9.418"}
    ]"#;

    #[test]
    fn parses_collection_json_with_real_shape() {
        let items = parse_collection(COLLECTION_FIXTURE);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].ideia_id, "227319");
        assert_eq!(
            items[0].titulo.as_deref(),
            Some("Disponibilização de Gasolina Pura")
        );
        // O ponto de milhar é do Senado e é preservado literalmente.
        assert_eq!(items[0].apoiamentos.as_deref(), Some("20.771"));
        assert_eq!(items[0].porcentagem_favor, Some(103));
        assert_eq!(items[1].ideia_id, "165188");
        assert_eq!(items[1].apoiamentos.as_deref(), Some("9.418"));
    }

    #[test]
    fn collection_parse_decodes_entities_and_survives_bad_items() {
        let json = r#"[
            {"id":1,"titulo":"Sa&uacute;de &amp; Educa&ccedil;&atilde;o","apoiamentos":"7"},
            {"titulo":"sem id"},
            {"id":"não numérico","titulo":"id inválido"},
            "string solta",
            {"id":2}
        ]"#;
        let items = parse_collection(json);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].ideia_id, "1");
        assert!(items[0].titulo.as_deref().unwrap().contains('&'));
        assert_eq!(items[0].apoiamentos.as_deref(), Some("7"));
        // Item só com id: candidata válida, título vem do fetch depois.
        assert_eq!(items[1].ideia_id, "2");
        assert!(items[1].titulo.is_none());
        assert!(!items[1].has_apoios());
    }

    #[test]
    fn collection_parse_is_empty_on_garbage() {
        assert!(parse_collection("não é json").is_empty());
        assert!(parse_collection(r#"{"erro":"objeto, não array"}"#).is_empty());
        assert!(parse_collection("[]").is_empty());
    }

    #[test]
    fn extracts_ideia_ids_from_principal_html() {
        let html = r#"<div class="lista">
            <a href="/ecidadania/visualizacaoideia?id=227319">Gasolina pura</a>
            <a href="https://www12.senado.leg.br/ecidadania/visualizacaoideia?id=165188#apoios">Piso</a>
            <a href="/ecidadania/visualizacaoideia?id=227319">duplicata ignorada</a>
            <a href="/ecidadania/visualizacaoideia?id=">sem id</a>
            <a href="/ecidadania/outracoisa?id=999">outra rota</a>
        </div>"#;
        assert_eq!(extract_ideia_ids(html), vec!["227319", "165188"]);
    }

    #[test]
    fn extract_ideia_ids_empty_without_links() {
        assert!(extract_ideia_ids("<html><body>nada aqui</body></html>").is_empty());
    }

    #[test]
    fn merge_prefers_collection_metadata_and_adds_html_only_ids() {
        let collection = parse_collection(COLLECTION_FIXTURE);
        let merged = merge_candidates(collection, vec!["227319".to_owned(), "999001".to_owned()]);
        // 227319 já vinha da coleção: não duplica e mantém o título/apoios.
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].ideia_id, "227319");
        assert_eq!(merged[0].apoiamentos.as_deref(), Some("20.771"));
        // O id só-HTML entra sem metadados.
        assert_eq!(merged[2], IdeaCandidate::bare("999001".to_owned()));
    }

    #[test]
    fn sweep_error_text_joins_and_clamps() {
        let clean = SweepStats::default();
        assert!(clean.error_text().is_none());
        let mut noisy = SweepStats {
            errors: vec!["a".repeat(400), "b".repeat(400)],
            ..SweepStats::default()
        };
        let text = noisy.error_text().expect("erros consolidados");
        assert_eq!(text.chars().count(), RUN_ERROR_MAX_CHARS);
        noisy.errors = vec!["coleção: timeout".to_owned(), "ideia 1: 404".to_owned()];
        assert_eq!(
            noisy.error_text().as_deref(),
            Some("coleção: timeout | ideia 1: 404")
        );
    }

    #[test]
    fn sweep_max_falls_back_to_default_on_bad_values() {
        assert_eq!(parse_sweep_max(Some(" 3 ")), 3);
        assert_eq!(parse_sweep_max(None), DEFAULT_SWEEP_MAX);
        assert_eq!(parse_sweep_max(Some("abc")), DEFAULT_SWEEP_MAX);
        // Teto 0 desligaria o sweep sem ninguém perceber.
        assert_eq!(parse_sweep_max(Some("0")), DEFAULT_SWEEP_MAX);
    }
}

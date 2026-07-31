//! # SOCRATES espelha Ideias Legislativas do e-Cidadania (migrations 0670/0671/0672).
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
//!    a ideia no endpoint JSON por id → cria o tópico assinado pelo bot.
//! 2. **Sweep automático** (v2, 0671): [`sweep_once`] lê a API pública JSON
//!    `restcolecaomaisideia` (as ideias EM ALTA, já com título e contador de
//!    apoios) e complementa com os ids linkados na página `principalideia`. As
//!    NOVAS viram tópico; as já espelhadas são RE-SINCRONIZADAS. Cada rodada
//!    vira uma linha em `socrates_sweep_run`.
//!
//! A coleção devolve ~5 itens e não pagina: o sweep é ACUMULATIVO por natureza
//! (roda a cada 6 h no worker e vai juntando o que o Senado promove ao topo),
//! e o teto `SOCRATES_SWEEP_MAX` impede que uma rodada anômala inunde o fórum.
//!
//! ## v3 (0672): a ideia INTEIRA, e viva
//!
//! Até a v2 o tópico tinha só o TÍTULO da ideia — o cidadão chegava no fórum
//! sem a proposta, sem o que debater. E o contador de apoios, embora
//! re-sincronizado no banco, era INVISÍVEL: o corpo do tópico era escrito UMA
//! vez na criação e nunca mais reescrito.
//!
//! A v3 corrige os dois com o endpoint JSON público POR IDEIA
//! ([`IDEIA_JSON_URL`]), que devolve a descrição integral (a PAUTA), o contador
//! de apoios como INTEIRO e a situação institucional da ideia. Ele SUBSTITUI o
//! scrape de `<title>`/`og:description`, que sobrevive só como fallback pra
//! quando o JSON falha. E [`refresh_mirrors`] REESCREVE o corpo dos tópicos já
//! espelhados quando apoios/situação/descrição mudam — é o que mantém os
//! números vivos. Só o `body` é tocado: score, votos e comentários do tópico
//! pertencem ao debate daqui, não à fonte.
//!
//! - `POST /admin/socrates/mirror`   — `{url_or_id}` → espelha (gate owner/admin).
//! - `GET  /admin/socrates/mirrors`  — lista os espelhos existentes.
//! - `POST /admin/socrates/sweep`    — dispara uma rodada agora (gate owner/admin).
//! - `GET  /admin/socrates/runs`     — log das últimas rodadas.
//! - `POST /admin/socrates/backfill` — reescreve o corpo de TODOS os espelhos
//!   (os criados antes da v3 ganham pauta/apoios/situação de uma vez).
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
/// API pública JSON (sem auth) de UMA ideia, por id. É a fonte canônica da v3:
/// devolve `descricao` (a pauta integral), `apoiamentos` como INTEIRO e
/// `situacaoIdeiaDescricao` — tudo que o HTML estático não dá.
const IDEIA_JSON_URL: &str = "https://www12.senado.leg.br/ecidadania/restideialegislativa?id=";
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
/// Teto da pauta copiada pro corpo do tópico. O limite dos fóruns é 40 000
/// caracteres pro corpo INTEIRO; cortar a descrição bem abaixo disso garante
/// que a moldura (link, atribuição, chamada ao debate) sempre cabe.
const MAX_DESCRICAO_CHARS: usize = 20_000;
/// Teto de espelhos re-sincronizados por rodada de sweep. Cada refresh custa um
/// fetch; o backfill (sob demanda do admin) é quem roda sem teto.
const SWEEP_REFRESH_MAX: i64 = 25;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/admin/socrates/mirror", post(mirror))
        .route("/admin/socrates/mirrors", get(list_mirrors))
        .route("/admin/socrates/sweep", post(run_sweep))
        .route("/admin/socrates/runs", get(list_runs))
        .route("/admin/socrates/backfill", post(backfill))
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

// ---------------------------------------------------------------------------
// Fonte canônica (v3): o endpoint JSON POR IDEIA
// ---------------------------------------------------------------------------

/// Uma Ideia Legislativa como o `restideialegislativa?id=` devolve.
///
/// É a fonte da PAUTA (`descricao`), do contador de apoios já numérico e da
/// situação institucional. Todos os campos são opcionais de propósito: o portal
/// é de terceiros e uma ideia com `detalhe` vazio (o caso comum) ou sem
/// situação não pode derrubar o espelho.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct IdeaDetail {
    titulo: Option<String>,
    /// O texto integral da proposta — o que o cidadão precisa pra debater.
    descricao: Option<String>,
    /// Contador de apoios como INTEIRO (a coleção dá "20.771"; aqui vem 20771).
    apoiamentos: Option<i64>,
    /// Situação institucional ("Convertida em Proposição", "Aguardando envio à CDH").
    situacao: Option<String>,
    /// Complemento da descrição. Costuma vir vazio.
    detalhe: Option<String>,
}

impl IdeaDetail {
    /// A pauta pro corpo do tópico: `descricao` e, quando existe, `detalhe`
    /// como parágrafo extra. `None` quando a ideia não tem texto nenhum — aí a
    /// seção "## A proposta" some do corpo em vez de virar cabeçalho órfão.
    fn pauta(&self) -> Option<String> {
        match (self.descricao.as_deref(), self.detalhe.as_deref()) {
            (Some(d), Some(extra)) => Some(format!("{d}\n\n{extra}")),
            (Some(d), None) => Some(d.to_owned()),
            (None, Some(extra)) => Some(extra.to_owned()),
            (None, None) => None,
        }
    }
}

/// URL canônica do JSON da ideia (host constante do módulo + id numérico já
/// validado — sem SSRF possível).
fn ideia_json_url(ideia_id: &str) -> String {
    format!("{IDEIA_JSON_URL}{ideia_id}")
}

/// Contador de apoios escrito como texto ("20.771"). O ponto é separador de
/// MILHAR no pt-BR, nunca decimal — por isso só os dígitos importam.
fn parse_apoios_text(raw: &str) -> Option<i64> {
    let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Um contador de apoios vindo do portal em qualquer das duas formas: inteiro
/// puro (endpoint por ideia) ou texto com ponto de milhar (coleção).
fn parse_apoios_num(value: &serde_json::Value) -> Option<i64> {
    match value {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => parse_apoios_text(s),
        _ => None,
    }
}

/// Parseia o JSON de UMA ideia. `None` = resposta fora do formato conhecido
/// (aí o chamador cai no fallback HTML em vez de espelhar lixo).
fn parse_idea_detail(json: &str) -> Option<IdeaDetail> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let obj = value.as_object()?;
    let text = |key: &str| {
        obj.get(key)
            .and_then(json_text)
            .map(|t| decode_entities(&t).trim().to_owned())
            .filter(|t| !t.is_empty())
    };
    let detail = IdeaDetail {
        titulo: text("titulo"),
        descricao: text("descricao"),
        apoiamentos: obj.get("apoiamentos").and_then(parse_apoios_num),
        situacao: text("situacaoIdeiaDescricao"),
        detalhe: text("detalhe"),
    };
    // Um objeto sem título NEM descrição não é uma ideia (404 em JSON, erro do
    // portal, shape novo): melhor cair no fallback do que espelhar vazio.
    (detail.titulo.is_some() || detail.descricao.is_some()).then_some(detail)
}

/// Busca a ideia no endpoint JSON canônico. `Err` = portal fora do ar,
/// status ruim ou shape irreconhecível.
async fn fetch_idea_detail(ideia_id: &str) -> Result<IdeaDetail, String> {
    let json = fetch_text(&ideia_json_url(ideia_id)).await?;
    parse_idea_detail(&json).ok_or_else(|| {
        "O e-Cidadania respondeu, mas o JSON da ideia veio fora do formato conhecido (a ideia \
         existe?)."
            .to_owned()
    })
}

/// Formata um inteiro com separador de milhar pt-BR: 20260 → "20.260".
fn format_milhar_ptbr(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let len = digits.len();
    let mut out = String::with_capacity(len + len / 3 + 1);
    if n < 0 {
        out.push('-');
    }
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push('.');
        }
        out.push(c);
    }
    out
}

/// O contador como o corpo do tópico deve exibi-lo: o inteiro do endpoint por
/// ideia manda (formatado em pt-BR); na falta dele, o texto que a coleção já
/// traz formatado pelo próprio Senado.
fn apoios_display(num: Option<i64>, texto: Option<&str>) -> Option<String> {
    num.map(format_milhar_ptbr)
        .or_else(|| texto.map(str::to_owned))
}

/// Trunca a pauta ao teto do corpo preservando caracteres inteiros.
fn clamp_pauta(pauta: &str) -> String {
    if pauta.chars().count() <= MAX_DESCRICAO_CHARS {
        return pauta.to_owned();
    }
    let mut t: String = pauta.chars().take(MAX_DESCRICAO_CHARS - 1).collect();
    t.push('…');
    t
}

/// Corpo (markdown) do tópico espelhado: a PAUTA integral, o placar de apoios e
/// a situação institucional, a chamada ao debate que o portal não permite, e a
/// atribuição da fonte.
///
/// Seção vazia é seção OMITIDA: sem pauta não há cabeçalho "## A proposta"
/// órfão, e sem apoios nem situação a linha do placar some inteira.
fn topic_body(
    url: &str,
    pauta: Option<&str>,
    apoios: Option<&str>,
    situacao: Option<&str>,
) -> String {
    let mut out =
        String::from("**Ideia Legislativa espelhada do e-Cidadania (Senado Federal)** 🏛️\n\n");
    if let Some(pauta) = pauta.map(str::trim).filter(|p| !p.is_empty()) {
        out.push_str("## A proposta\n");
        out.push_str(&clamp_pauta(pauta));
        out.push_str("\n\n");
    }
    // As duas metades do placar são independentes: o Senado às vezes dá uma sem
    // a outra, e meia linha ainda informa.
    let placar = match (apoios, situacao) {
        (Some(a), Some(s)) => Some(format!(
            "📊 **{a} apoios** no e-Cidadania · **Situação:** {s}"
        )),
        (Some(a), None) => Some(format!("📊 **{a} apoios** no e-Cidadania")),
        (None, Some(s)) => Some(format!("📊 **Situação:** {s}")),
        (None, None) => None,
    };
    if let Some(placar) = placar {
        out.push_str(&placar);
        out.push_str("\n\n");
    }
    out.push_str(
        "---\n\
         No portal do Senado, cidadãos só podem **apoiar** esta ideia — não há como argumentar \
         contra, ponderar ou debater. Aqui no DemocraciaBR o debate é completo: **argumente a \
         favor ou contra e vote**.\n\n",
    );
    out.push_str(&format!(
        "📌 Ideia original: {url}\n\
         (Ao atingir 20.000 apoios no e-Cidadania, a ideia vira sugestão legislativa formal no \
         Senado.)\n\n"
    ));
    out.push_str(
        "— *SOCRATES, agente cívico da plataforma. Conteúdo público do Senado Federal, \
         espelhado com atribuição.*",
    );
    out
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
    /// Contador de apoios como NÚMERO (v3) — o painel formata em pt-BR.
    apoiamentos_num: Option<i64>,
    /// Situação institucional da ideia no Senado.
    situacao: Option<String>,
    /// Quando o corpo do tópico foi reescrito com os dados acima. `null` = o
    /// tópico ainda está no formato pré-v3 (só título) e precisa de backfill.
    body_synced_at: Option<DateTime<Utc>>,
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
    /// Espelhos existentes que tiveram o contador de apoios re-sincronizado a
    /// partir da COLEÇÃO (sem fetch extra — é de lá que vem `porcentagem_favor`).
    pub updated: usize,
    /// Espelhos cujo CORPO do tópico foi reescrito com pauta/apoios/situação
    /// frescos do endpoint por ideia (v3) — o que mantém os números visíveis.
    pub refreshed: usize,
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

/// Espelha UMA ideia: dedup → busca a ideia no JSON canônico → tópico + linha
/// `socrates_mirror` na MESMA transação. É o núcleo compartilhado pelos dois
/// caminhos (painel admin e sweep); devolve o id do tópico criado.
///
/// O tópico já nasce COMPLETO — pauta, apoios e situação no corpo. Quando o
/// JSON canônico falha, o espelho ainda acontece com o que houver (título da
/// coleção ou scrape do HTML) e fica com `body_synced_at` NULL, o que faz o
/// refresh/backfill completá-lo na primeira oportunidade.
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

    // Fonte canônica (v3): o JSON por ideia traz título + pauta + apoios +
    // situação de uma vez. Falha dele NÃO cancela o espelho — o tópico nasce
    // com o que houver (título da coleção ou scrape do HTML) e o refresh
    // completa depois, já que `body_synced_at` fica NULL.
    let detail = match fetch_idea_detail(ideia_id).await {
        Ok(detail) => Some(detail),
        Err(msg) => {
            tracing::warn!(ideia_id, msg, "socrates: JSON da ideia indisponível");
            None
        }
    };

    let known_title = detail
        .as_ref()
        .and_then(|d| d.titulo.clone())
        .or_else(|| candidate.titulo.clone());
    let title = match known_title {
        Some(title) => title,
        None => {
            // Último recurso: o `<title>`/`og:description` server-rendered.
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

    let pauta = detail.as_ref().and_then(IdeaDetail::pauta);
    let situacao = detail.as_ref().and_then(|d| d.situacao.clone());
    // O inteiro do endpoint por ideia manda; na falta dele, o texto da coleção
    // ("20.771") vira número — as duas fontes alimentam a mesma coluna.
    let apoios_num = detail
        .as_ref()
        .and_then(|d| d.apoiamentos)
        .or_else(|| candidate.apoiamentos.as_deref().and_then(parse_apoios_text));
    let apoios_texto = apoios_display(apoios_num, candidate.apoiamentos.as_deref());
    let body = topic_body(
        &source_url,
        pauta.as_deref(),
        apoios_texto.as_deref(),
        situacao.as_deref(),
    );
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
              apoiamentos, porcentagem_favor, apoios_updated_at, origin,
              descricao, situacao, apoiamentos_num, body_synced_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
         ON CONFLICT (ideia_id) DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(ideia_id)
    .bind(&source_url)
    .bind(topic.id)
    .bind(now)
    .bind(apoios_texto.as_deref())
    .bind(candidate.porcentagem_favor)
    .bind((apoios_num.is_some() || candidate.has_apoios()).then_some(now))
    .bind(origin)
    .bind(pauta.as_deref())
    .bind(situacao.as_deref())
    .bind(apoios_num)
    // NULL quando o JSON canônico falhou: o corpo nasceu incompleto e o
    // refresh/backfill vai reescrevê-lo na primeira oportunidade.
    .bind(detail.is_some().then_some(now))
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
    /// A linha crua do JOIN; o DTO só acrescenta o `path` navegável.
    #[derive(sqlx::FromRow)]
    struct Row {
        ideia_id: String,
        source_url: String,
        topic_id: Uuid,
        topic_title: String,
        created_at: DateTime<Utc>,
        apoiamentos: Option<String>,
        porcentagem_favor: Option<i32>,
        apoios_updated_at: Option<DateTime<Utc>>,
        origin: String,
        apoiamentos_num: Option<i64>,
        situacao: Option<String>,
        body_synced_at: Option<DateTime<Utc>>,
    }
    let rows: Result<Vec<Row>, sqlx::Error> = sqlx::query_as(
        "SELECT m.ideia_id, m.source_url, m.topic_id, t.title AS topic_title, m.created_at,
                m.apoiamentos, m.porcentagem_favor, m.apoios_updated_at, m.origin,
                m.apoiamentos_num, m.situacao, m.body_synced_at
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
                .map(|r| MirrorEntry {
                    path: format!("/f/topico/{}", r.topic_id),
                    ideia_id: r.ideia_id,
                    source_url: r.source_url,
                    topic_id: r.topic_id,
                    topic_title: r.topic_title,
                    created_at: r.created_at,
                    apoiamentos: r.apoiamentos,
                    porcentagem_favor: r.porcentagem_favor,
                    apoios_updated_at: r.apoios_updated_at,
                    origin: r.origin,
                    apoiamentos_num: r.apoiamentos_num,
                    situacao: r.situacao,
                    body_synced_at: r.body_synced_at,
                })
                .collect();
            (StatusCode::OK, Json(ApiResponse::ok(dtos))).into_response()
        }
        Err(err) => {
            tracing::error!(?err, "socrates: listagem falhou");
            storage_error()
        }
    }
}

/// `POST /admin/socrates/backfill` — re-sincroniza TODOS os espelhos: busca
/// cada ideia no JSON canônico e reescreve o corpo do tópico com pauta, apoios
/// e situação. É o que conserta os espelhos criados antes da v3 (corpo só com
/// título) e os que ficaram com `apoiamentos` NULL por terem vindo do HTML.
///
/// Síncrono e sem teto de propósito: são poucos espelhos e o admin quer ver o
/// resultado. Idempotente — rodar duas vezes não reescreve nada na segunda.
async fn backfill(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_admin(&state.db, &headers).await {
        return resp;
    }
    let stats = refresh_mirrors(&state, None).await;
    (StatusCode::OK, Json(ApiResponse::ok(stats))).into_response()
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

    // v3: re-sincroniza o CORPO dos espelhos (inclusive os que não apareceram
    // nas fontes desta rodada — apoios mudam mesmo em ideia fora do topo).
    // Os mais desatualizados primeiro, até o teto.
    let refresh = refresh_mirrors(state, Some(SWEEP_REFRESH_MAX)).await;
    stats.refreshed = refresh.refreshed;
    stats.errors.extend(refresh.errors);

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

// ---------------------------------------------------------------------------
// Refresh (v3): manter a ideia espelhada VIVA
// ---------------------------------------------------------------------------
//
// O bug que isto conserta: o corpo do tópico era escrito UMA vez, na criação.
// O sweep atualizava `socrates_mirror.apoiamentos` e o número continuava
// invisível no fórum. Aqui o corpo é REESCRITO quando a fonte muda — e só o
// corpo: score, votos e comentários pertencem ao debate daqui.

/// O que o refresh precisa saber sobre um espelho pra decidir se reescreve.
#[derive(Debug, Clone)]
struct MirrorRow {
    ideia_id: String,
    topic_id: Uuid,
    source_url: String,
    descricao: Option<String>,
    situacao: Option<String>,
    apoiamentos_num: Option<i64>,
    apoiamentos: Option<String>,
    /// `None` = o tópico ainda tem o corpo pré-v3 (só título): reescreve sempre.
    body_synced_at: Option<DateTime<Utc>>,
}

/// Os espelhos a re-sincronizar, os mais desatualizados primeiro (`NULLS FIRST`
/// põe na frente exatamente os que nunca tiveram o corpo preenchido).
/// `limit = None` = todos (o caminho do backfill).
async fn load_mirror_rows(db: &PgPool, limit: Option<i64>) -> Result<Vec<MirrorRow>, sqlx::Error> {
    type Row = (
        String,
        Uuid,
        String,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<String>,
        Option<DateTime<Utc>>,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT ideia_id, topic_id, source_url, descricao, situacao,
                apoiamentos_num, apoiamentos, body_synced_at
           FROM socrates_mirror
          ORDER BY body_synced_at ASC NULLS FIRST
          LIMIT $1",
    )
    // `NULL` no LIMIT do Postgres significa "sem limite" — é o backfill.
    .bind(limit)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                ideia_id,
                topic_id,
                source_url,
                descricao,
                situacao,
                apoiamentos_num,
                apoiamentos,
                body_synced_at,
            )| MirrorRow {
                ideia_id,
                topic_id,
                source_url,
                descricao,
                situacao,
                apoiamentos_num,
                apoiamentos,
                body_synced_at,
            },
        )
        .collect())
}

/// Re-sincroniza UM espelho: busca a ideia no JSON canônico e, se algo mudou
/// (ou se o corpo nunca foi preenchido), reescreve o `body` do tópico e grava
/// pauta/situação/apoios. `Ok(false)` = nada mudou, nada foi escrito.
async fn refresh_mirror(
    state: &AppState,
    row: &MirrorRow,
    now: DateTime<Utc>,
) -> Result<bool, String> {
    let detail = fetch_idea_detail(&row.ideia_id).await?;
    let pauta = detail.pauta();
    let situacao = detail.situacao.clone();
    let apoios_num = detail
        .apoiamentos
        .or_else(|| row.apoiamentos.as_deref().and_then(parse_apoios_text));

    // Nada mudou E o corpo já foi escrito pela v3: não toca no tópico (evita
    // reescrever 11 tópicos a cada 6 h sem motivo).
    let unchanged = row.body_synced_at.is_some()
        && pauta.as_deref() == row.descricao.as_deref()
        && situacao.as_deref() == row.situacao.as_deref()
        && apoios_num == row.apoiamentos_num;
    if unchanged {
        return Ok(false);
    }

    let apoios_texto = apoios_display(apoios_num, row.apoiamentos.as_deref());
    let body = topic_body(
        &row.source_url,
        pauta.as_deref(),
        apoios_texto.as_deref(),
        situacao.as_deref(),
    );

    let mut tx = state.db.begin().await.map_err(|err| {
        tracing::error!(?err, "socrates refresh: begin falhou");
        "Erro ao abrir a transação do refresh.".to_owned()
    })?;
    // SÓ o corpo: `score`, `comment_count` e votos são do debate daqui.
    sqlx::query("UPDATE forum_topic SET body = $2 WHERE id = $1")
        .bind(row.topic_id)
        .bind(&body)
        .execute(&mut *tx)
        .await
        .map_err(|err| {
            tracing::error!(?err, ideia_id = %row.ideia_id, "socrates refresh: UPDATE do corpo falhou");
            "Erro ao reescrever o corpo do tópico.".to_owned()
        })?;
    sqlx::query(
        "UPDATE socrates_mirror
            SET descricao = $2, situacao = $3, apoiamentos_num = $4,
                apoiamentos = $5, apoios_updated_at = $6, body_synced_at = $6
          WHERE ideia_id = $1",
    )
    .bind(&row.ideia_id)
    .bind(pauta.as_deref())
    .bind(situacao.as_deref())
    .bind(apoios_num)
    .bind(apoios_texto.as_deref())
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(|err| {
        tracing::error!(?err, ideia_id = %row.ideia_id, "socrates refresh: UPDATE do espelho falhou");
        "Erro ao gravar os dados do espelho.".to_owned()
    })?;
    tx.commit().await.map_err(|err| {
        tracing::error!(?err, "socrates refresh: commit falhou");
        "Erro ao concluir o refresh.".to_owned()
    })?;
    Ok(true)
}

/// Quantos espelhos foram VISTOS e quantos foram efetivamente reescritos.
#[derive(Debug, Default, Clone, Serialize)]
pub struct RefreshStats {
    /// Espelhos considerados nesta passada.
    pub total: usize,
    /// Espelhos cujo corpo foi reescrito (apoios/situação/pauta mudaram).
    pub refreshed: usize,
    /// Falhas por ideia — a passada segue com as demais.
    pub errors: Vec<String>,
}

/// Re-sincroniza os espelhos. `limit = None` = todos (backfill); com teto, os
/// mais desatualizados primeiro (sweep). Uma ideia que falha não derruba a
/// passada: o erro entra na lista e a próxima segue.
async fn refresh_mirrors(state: &AppState, limit: Option<i64>) -> RefreshStats {
    let mut stats = RefreshStats::default();
    let rows = match load_mirror_rows(&state.db, limit).await {
        Ok(rows) => rows,
        Err(err) => {
            tracing::error!(?err, "socrates refresh: leitura dos espelhos falhou");
            stats
                .errors
                .push("Erro ao ler os espelhos existentes.".to_owned());
            return stats;
        }
    };
    stats.total = rows.len();
    let now = state.clock.now();
    for row in &rows {
        match refresh_mirror(state, row, now).await {
            Ok(true) => stats.refreshed += 1,
            Ok(false) => {}
            Err(msg) => stats.errors.push(format!("ideia {}: {msg}", row.ideia_id)),
        }
    }
    stats
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

    // --- v3: fonte canônica (JSON por ideia), formatação e corpo do tópico ---
    // Fixture inline com o shape REAL observado ao vivo; o Senado nunca é
    // chamado em teste.

    /// Shape real de `GET /ecidadania/restideialegislativa?id=212832`.
    const DETAIL_FIXTURE: &str = r#"{
        "situacaoIdeiaId":10,
        "situacaoIdeiaDescricao":"Convertida em Proposição",
        "descricao":"Em tempos de paz, será vedado o Militar das Forças Armadas após o término do seu serviço de 24h Armado, cumprir o Expediente.",
        "titulo":"Regulamentação da Carga Horária de Militares das Forças Armadas em tempos de paz.",
        "apoiamentos":20260,
        "detalhe":"",
        "ideiaId":212832
    }"#;

    #[test]
    fn parses_idea_detail_with_real_shape() {
        let d = parse_idea_detail(DETAIL_FIXTURE).expect("ideia parseada");
        assert_eq!(
            d.titulo.as_deref(),
            Some(
                "Regulamentação da Carga Horária de Militares das Forças Armadas em tempos de paz."
            )
        );
        assert!(d
            .descricao
            .as_deref()
            .expect("pauta")
            .starts_with("Em tempos de paz, será vedado o Militar"));
        // O endpoint por ideia dá INTEIRO puro (a coleção daria "20.260").
        assert_eq!(d.apoiamentos, Some(20_260));
        assert_eq!(d.situacao.as_deref(), Some("Convertida em Proposição"));
        // `detalhe` vazio é ausente, não string vazia.
        assert!(d.detalhe.is_none());
        // Sem `detalhe`, a pauta é a descrição pura.
        assert_eq!(d.pauta(), d.descricao);
    }

    #[test]
    fn idea_detail_decodes_entities_and_merges_detalhe() {
        let json = r#"{"titulo":"Sa&uacute;de &amp; Educa&ccedil;&atilde;o","descricao":"Corpo da &quot;pauta&quot;",
                       "detalhe":"Observação complementar.","apoiamentos":"1.234",
                       "situacaoIdeiaDescricao":"Aguardando envio à CDH"}"#;
        let d = parse_idea_detail(json).expect("ideia parseada");
        assert_eq!(d.descricao.as_deref(), Some("Corpo da \"pauta\""));
        assert!(d.titulo.as_deref().expect("título").contains('&'));
        // Apoios como texto com ponto de milhar viram o mesmo inteiro.
        assert_eq!(d.apoiamentos, Some(1_234));
        assert_eq!(d.situacao.as_deref(), Some("Aguardando envio à CDH"));
        // Com `detalhe`, a pauta vira descrição + parágrafo extra.
        assert_eq!(
            d.pauta().as_deref(),
            Some("Corpo da \"pauta\"\n\nObservação complementar.")
        );
    }

    #[test]
    fn idea_detail_is_none_on_garbage_or_empty_idea() {
        assert!(parse_idea_detail("não é json").is_none());
        assert!(parse_idea_detail("[1,2,3]").is_none());
        // Objeto sem título NEM descrição não é uma ideia (404 em JSON, p.ex.).
        assert!(parse_idea_detail(r#"{"apoiamentos":10,"detalhe":""}"#).is_none());
        // Só a descrição já basta: o título vem do fallback.
        assert!(parse_idea_detail(r#"{"descricao":"só a pauta"}"#).is_some());
    }

    #[test]
    fn formats_thousands_in_ptbr() {
        assert_eq!(format_milhar_ptbr(20_260), "20.260");
        assert_eq!(format_milhar_ptbr(0), "0");
        assert_eq!(format_milhar_ptbr(999), "999");
        assert_eq!(format_milhar_ptbr(1_000), "1.000");
        assert_eq!(format_milhar_ptbr(1_234_567), "1.234.567");
        assert_eq!(format_milhar_ptbr(-1_500), "-1.500");
    }

    #[test]
    fn apoios_display_prefers_the_number_over_senado_text() {
        assert_eq!(
            apoios_display(Some(20_260), Some("valor velho")).as_deref(),
            Some("20.260")
        );
        // Sem número, o texto que o Senado já formatou serve.
        assert_eq!(
            apoios_display(None, Some("20.771")).as_deref(),
            Some("20.771")
        );
        assert!(apoios_display(None, None).is_none());
    }

    #[test]
    fn topic_body_carries_pauta_placar_and_attribution() {
        let b = topic_body(
            "https://exemplo/id=1",
            Some("A proposta integral vem aqui."),
            Some("20.260"),
            Some("Convertida em Proposição"),
        );
        assert!(b.contains("## A proposta\nA proposta integral vem aqui."));
        assert!(b.contains(
            "📊 **20.260 apoios** no e-Cidadania · **Situação:** Convertida em Proposição"
        ));
        assert!(b.contains("argumente a favor ou contra"));
        assert!(b.contains("📌 Ideia original: https://exemplo/id=1"));
        assert!(b.contains("Ao atingir 20.000 apoios"));
        assert!(b.contains("SOCRATES"));
    }

    #[test]
    fn topic_body_omits_empty_sections_without_orphan_headings() {
        // Sem pauta: nada de cabeçalho "## A proposta" órfão.
        let b = topic_body("https://exemplo/id=1", None, Some("9.418"), None);
        assert!(!b.contains("## A proposta"));
        assert!(b.contains("📊 **9.418 apoios** no e-Cidadania"));
        assert!(!b.contains("Situação:"));

        // Descrição só de espaços conta como ausente.
        let b = topic_body("https://exemplo/id=1", Some("   "), None, None);
        assert!(!b.contains("## A proposta"));
        // Sem apoios NEM situação, a linha do placar some inteira.
        assert!(!b.contains("📊"));
        // A moldura (chamada ao debate + atribuição) nunca some.
        assert!(b.contains("argumente a favor ou contra"));
        assert!(b.contains("SOCRATES"));

        // Só a situação: meia linha ainda informa.
        let b = topic_body(
            "https://exemplo/id=1",
            None,
            None,
            Some("Aguardando envio à CDH"),
        );
        assert!(b.contains("📊 **Situação:** Aguardando envio à CDH"));
    }

    #[test]
    fn topic_body_stays_within_the_forum_body_limit() {
        // Uma pauta absurda não pode reprovar na validação dos fóruns.
        let pauta = "á".repeat(MAX_DESCRICAO_CHARS * 3);
        let b = topic_body(
            "https://exemplo/id=1",
            Some(&pauta),
            Some("1"),
            Some("Situação"),
        );
        assert!(b.chars().count() <= dsoc_forums::domain::MAX_BODY_LEN);
        assert!(dsoc_forums::domain::NewTopic::validate("Título", &b).is_ok());
        assert!(b.contains('…'));
    }

    #[test]
    fn ideia_json_url_pins_senado_host() {
        assert_eq!(
            ideia_json_url("212832"),
            "https://www12.senado.leg.br/ecidadania/restideialegislativa?id=212832"
        );
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

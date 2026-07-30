//! # SOCRATES espelha Ideias Legislativas do e-Cidadania (migration 0670).
//!
//! No portal e-Cidadania do Senado o cidadão SÓ pode apoiar uma Ideia
//! Legislativa — não existe votar contra nem argumentar. Aqui o debate é
//! completo (favor × contra + afirmação-ponte), então o SOCRATES (cidadão-bot
//! institucional, UUID fixo em 0670) espelha a ideia como tópico do fórum
//! `senado` pra abrir o debate que o portal não permite. MVP ADMIN-CURADO:
//! o admin cola a URL/ID → o gateway busca o título server-rendered
//! (`<title>` / `og:description`; a descrição longa é SPA e NÃO vem no HTML
//! estático) → cria o tópico assinado pelo bot + grava o espelho, na MESMA
//! transação.
//!
//! - `POST /admin/socrates/mirror`  — `{url_or_id}` → espelha (gate owner/admin).
//! - `GET  /admin/socrates/mirrors` — lista os espelhos existentes.
//!
//! Autor-bot: o `create_topic` HTTP dos fóruns exige caller verificado; aqui o
//! gateway insere via `dsoc_forums::queries::insert_topic` DIRETO (mesma query
//! do service, contadores default idênticos), dentro de uma transação junto com
//! a linha `socrates_mirror` — atômico e sem passar pelo gate de sessão.

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
fn topic_body(title: &str, url: &str) -> String {
    format!(
        "**Ideia Legislativa espelhada do e-Cidadania (Senado Federal)** 🏛️\n\n\
         > {title}\n\n\
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

    // Dedup ANTES do fetch: ideia já espelhada nunca dispara rede.
    match sqlx::query_scalar::<_, Uuid>("SELECT topic_id FROM socrates_mirror WHERE ideia_id = $1")
        .bind(&ideia_id)
        .fetch_optional(&state.db)
        .await
    {
        Ok(Some(existing)) => return already_mirrored(&ideia_id, existing),
        Ok(None) => {}
        Err(err) => {
            tracing::error!(?err, "socrates: dedup lookup falhou");
            return storage_error();
        }
    }

    let source_url = canonical_url(&ideia_id);
    let html = match fetch_ideia(&source_url).await {
        Ok(html) => html,
        Err(msg) => return fail(StatusCode::BAD_GATEWAY, "upstream_error", &msg),
    };
    let Some(title) = extract_title(&html) else {
        return fail(
            StatusCode::BAD_GATEWAY,
            "upstream_error",
            "A página do e-Cidadania respondeu, mas não foi possível extrair o título da ideia \
             (formato inesperado — a ideia existe?).",
        );
    };
    let title = clamp_title(&title);
    let body = topic_body(&title, &source_url);
    let new = match dsoc_forums::domain::NewTopic::validate(&title, &body) {
        Ok(n) => n,
        Err(err) => {
            tracing::error!(?err, "socrates: título/corpo fora dos limites dos fóruns");
            return fail(
                StatusCode::BAD_GATEWAY,
                "upstream_error",
                "O título extraído do e-Cidadania não passou na validação dos fóruns.",
            );
        }
    };

    // Fórum destino: a raiz `senado` (institucional, semeada — nunca é
    // materializável on-demand como as seções territoriais).
    let forum_id: Option<Uuid> =
        match sqlx::query_scalar("SELECT id FROM forum WHERE org_id = $1 AND full_path = $2")
            .bind(DEFAULT_ORG_UUID)
            .bind(SENADO_FORUM_PATH)
            .fetch_optional(&state.db)
            .await
        {
            Ok(f) => f,
            Err(err) => {
                tracing::error!(?err, "socrates: lookup do fórum senado falhou");
                return storage_error();
            }
        };
    let Some(forum_id) = forum_id else {
        return fail(
            StatusCode::INTERNAL_SERVER_ERROR,
            "forum_missing",
            "O fórum 'senado' não existe nesta instalação (rode scripts/seed-forums.sql).",
        );
    };

    // Tópico + espelho na MESMA transação: ou os dois existem, ou nenhum.
    // `insert_topic` é a MESMA query do ForumService::create_topic (contadores
    // default idênticos); inserir direto evita o gate de sessão/verificação do
    // caminho HTTP — o autor é o cidadão-bot, sem credencial.
    let now = state.clock.now();
    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(err) => {
            tracing::error!(?err, "socrates: begin falhou");
            return storage_error();
        }
    };
    let topic = match dsoc_forums::queries::insert_topic(
        &mut *tx,
        Uuid::now_v7(),
        forum_id,
        SOCRATES_CITIZEN_ID,
        &new.title,
        &new.body,
        now,
    )
    .await
    {
        Ok(t) => t,
        Err(err) => {
            tracing::error!(?err, "socrates: insert_topic falhou");
            return storage_error();
        }
    };
    let inserted = sqlx::query(
        "INSERT INTO socrates_mirror (id, ideia_id, source_url, topic_id, created_at)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (ideia_id) DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(&ideia_id)
    .bind(&source_url)
    .bind(topic.id)
    .bind(now)
    .execute(&mut *tx)
    .await;
    match inserted {
        // Corrida: outro admin espelhou entre o dedup e o commit — aborta o
        // tópico (rollback implícito no drop da tx) e responde 409.
        Ok(r) if r.rows_affected() == 0 => {
            drop(tx);
            match sqlx::query_scalar::<_, Uuid>(
                "SELECT topic_id FROM socrates_mirror WHERE ideia_id = $1",
            )
            .bind(&ideia_id)
            .fetch_optional(&state.db)
            .await
            {
                Ok(Some(existing)) => return already_mirrored(&ideia_id, existing),
                _ => return storage_error(),
            }
        }
        Ok(_) => {}
        Err(err) => {
            tracing::error!(?err, "socrates: insert do espelho falhou");
            return storage_error();
        }
    }
    if let Err(err) = tx.commit().await {
        tracing::error!(?err, "socrates: commit falhou");
        return storage_error();
    }

    let dto = MirrorCreated {
        topic_id: topic.id,
        path: format!("/f/topico/{}", topic.id),
    };
    (StatusCode::CREATED, Json(ApiResponse::ok(dto))).into_response()
}

/// `GET /admin/socrates/mirrors` — os espelhos existentes, mais recentes primeiro.
async fn list_mirrors(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_admin(&state.db, &headers).await {
        return resp;
    }
    let rows: Result<Vec<(String, String, Uuid, String, DateTime<Utc>)>, sqlx::Error> =
        sqlx::query_as(
            "SELECT m.ideia_id, m.source_url, m.topic_id, t.title, m.created_at
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
                    |(ideia_id, source_url, topic_id, topic_title, created_at)| MirrorEntry {
                        ideia_id,
                        source_url,
                        topic_id,
                        topic_title,
                        path: format!("/f/topico/{topic_id}"),
                        created_at,
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

/// GET simples na página da ideia (server-rendered basta pro título).
async fn fetch_ideia(url: &str) -> Result<String, String> {
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
        let b = topic_body("Título X", "https://exemplo/id=1");
        assert!(b.contains("> Título X"));
        assert!(b.contains("https://exemplo/id=1"));
        assert!(b.contains("SOCRATES"));
        assert!(b.contains("20.000 apoios"));
    }
}

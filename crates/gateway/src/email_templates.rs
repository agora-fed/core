//! Templates de e-mail editáveis (0.25.0-fediverso).
//!
//! Todos os e-mails da plataforma passam por aqui: o corpo é lido de
//! `email_template` no DB, `{{var}}` é substituído server-side com o contexto,
//! e o resultado vai pro SMTP. Admin edita subject/body pela UI em
//! `/admin/email-templates`; volta ao padrão via `PATCH /admin/email-templates/:key
//! {reset: true}` (subject/body ← default_subject/default_body).
//!
//! Sintaxe do template: apenas `{{var_name}}`. Sem loops, sem if, sem escape
//! HTML (todos os e-mails são text/plain). Um placeholder desconhecido fica
//! literal `{{foo}}` na saída — sinaliza pro admin que a variável está errada.

use axum::extract::{Json, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch};
use axum::Router;
use dsoc_api_contract::ApiResponse;
use dsoc_app::AppState;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Rendering — chamado pelas crates que enviam e-mail.
// ---------------------------------------------------------------------------

/// Busca o template `key` no DB e renderiza subject/body com o contexto.
/// Retorna `(subject_final, body_final)` ou `None` se a chave não existe
/// (o caller cai no fallback hardcoded).
pub async fn render(
    db: &PgPool,
    key: &str,
    vars: &HashMap<&str, String>,
) -> Option<(String, String)> {
    let row = match sqlx::query_as::<_, TemplateRow>(
        r"SELECT key, label, subject, body, default_subject, default_body,
                 variables, updated_at
            FROM email_template WHERE key = $1",
    )
    .bind(key)
    .fetch_optional(db)
    .await
    {
        Ok(Some(r)) => r,
        Ok(None) => return None,
        Err(err) => {
            tracing::warn!(?err, key, "email_template: lookup falhou");
            return None;
        }
    };
    let subject = if row.subject.trim().is_empty() {
        &row.default_subject
    } else {
        &row.subject
    };
    let body = if row.body.trim().is_empty() {
        &row.default_body
    } else {
        &row.body
    };
    Some((substitute(subject, vars), substitute(body, vars)))
}

/// Substitui `{{var_name}}` no texto pelo valor no HashMap. Variáveis
/// não encontradas ficam literais `{{var_name}}` — sinaliza pro admin que
/// o template referencia um placeholder que o code path não injeta.
fn substitute(input: &str, vars: &HashMap<&str, String>) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == '{' && input[i..].starts_with("{{") {
            if let Some(end) = input[i + 2..].find("}}") {
                let key = input[i + 2..i + 2 + end].trim();
                match vars.get(key) {
                    Some(val) => out.push_str(val),
                    None => out.push_str(&input[i..i + 2 + end + 2]),
                }
                // pula até `}}` inclusive
                for _ in 0..(end + 3) {
                    chars.next();
                }
                continue;
            }
        }
        out.push(c);
    }
    out
}

// ---------------------------------------------------------------------------
// HTTP surface — admin CRUD
// ---------------------------------------------------------------------------

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/admin/email-templates", get(list))
        .route("/admin/email-templates/{key}", patch(update))
        .route("/admin/email-templates/{key}/preview", axum::routing::post(preview))
        // GET /me/admin-status — usado pelo AuthMenu no front pra saber se
        // mostra o link "Administração" no dropdown do perfil. Anônimo → 200
        // com `{is_admin: false}` (não vaza sinal). Não é aqui só porque o
        // path começa com /me em vez de /admin — casa com require_admin abaixo.
        .route("/me/admin-status", get(me_admin_status))
        .with_state(state)
}

/// `GET /me/admin-status` — leve, o AuthMenu chama uma vez no login e cacheia
/// no `dsoc_is_admin` do localStorage. Retorna `{is_admin: bool}`. Não é 401
/// pra anônimo — devolve `false`, evita blip de erro no console pra usuário
/// não-admin.
async fn me_admin_status(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(citizen_id): Option<Uuid> = headers
        .get("x-dsoc-citizen-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
    else {
        return (
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({ "is_admin": false }))),
        )
            .into_response();
    };
    let is_admin = sqlx::query_scalar::<_, bool>(
        r"SELECT EXISTS (
             SELECT 1 FROM admin_role_binding
              WHERE citizen_id = $1 AND role IN ('owner','admin')
           )",
    )
    .bind(citizen_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(false);
    (
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({ "is_admin": is_admin }))),
    )
        .into_response()
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct TemplateRow {
    key: String,
    label: String,
    subject: String,
    body: String,
    default_subject: String,
    default_body: String,
    variables: Vec<String>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

async fn require_admin(headers: &HeaderMap, db: &PgPool) -> std::result::Result<Uuid, Response> {
    let citizen_id: Uuid = headers
        .get("x-dsoc-citizen-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(ApiResponse::<()>::fail("unauthorized", "Autenticação necessária.")),
            )
                .into_response()
        })?;
    // admin_role_binding tem role IN ('owner','admin','auditor') — owner/admin podem
    // editar; auditor é read-only (ainda não implementado aqui).
    let is_admin: Option<bool> = sqlx::query_scalar(
        r"SELECT EXISTS (
             SELECT 1 FROM admin_role_binding
              WHERE citizen_id = $1 AND role IN ('owner','admin')
           )",
    )
    .bind(citizen_id)
    .fetch_optional(db)
    .await
    .unwrap_or(None);
    if !matches!(is_admin, Some(true)) {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ApiResponse::<()>::fail("forbidden", "Acesso restrito a admins.")),
        )
            .into_response());
    }
    Ok(citizen_id)
}

async fn list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_admin(&headers, &state.db).await {
        return resp;
    }
    match sqlx::query_as::<_, TemplateRow>(
        r"SELECT key, label, subject, body, default_subject, default_body,
                 variables, updated_at
            FROM email_template ORDER BY key",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => (StatusCode::OK, Json(ApiResponse::ok(rows))).into_response(),
        Err(err) => {
            tracing::error!(?err, "email_template list falhou");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::fail("storage_error", "Erro interno.")),
            )
                .into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct UpdateBody {
    /// `Some("")` limpa (usa default). `None` deixa como está.
    subject: Option<String>,
    body: Option<String>,
    /// `Some(true)` reseta subject+body pros defaults.
    #[serde(default)]
    reset: bool,
}

async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    Json(body): Json<UpdateBody>,
) -> Response {
    let admin_id = match require_admin(&headers, &state.db).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let res = if body.reset {
        sqlx::query(
            r"UPDATE email_template
                 SET subject = default_subject,
                     body    = default_body,
                     updated_at = now(),
                     updated_by = $2
               WHERE key = $1",
        )
        .bind(&key)
        .bind(admin_id)
        .execute(&state.db)
        .await
    } else {
        sqlx::query(
            r"UPDATE email_template
                 SET subject = COALESCE($2, subject),
                     body    = COALESCE($3, body),
                     updated_at = now(),
                     updated_by = $4
               WHERE key = $1",
        )
        .bind(&key)
        .bind(&body.subject)
        .bind(&body.body)
        .bind(admin_id)
        .execute(&state.db)
        .await
    };
    match res {
        Ok(r) if r.rows_affected() == 0 => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::fail("not_found", "Template não existe.")),
        )
            .into_response(),
        Ok(_) => (StatusCode::OK, Json(ApiResponse::<()>::ok(()))).into_response(),
        Err(err) => {
            tracing::error!(?err, key, "email_template update falhou");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::fail("storage_error", "Erro interno.")),
            )
                .into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct PreviewBody {
    /// Contexto arbitrário `{var_name: valor}`. UI passa os valores que o
    /// admin quer testar; ausente vira `{{var_name}}` literal na saída.
    #[serde(default)]
    context: HashMap<String, String>,
    /// Se `Some(...)`, renderiza esse subject/body draft (antes de salvar).
    /// Se `None`, renderiza o que está salvo hoje.
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    body: Option<String>,
}

#[derive(Debug, Serialize)]
struct PreviewResult {
    subject: String,
    body: String,
}

async fn preview(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    Json(body): Json<PreviewBody>,
) -> Response {
    if let Err(resp) = require_admin(&headers, &state.db).await {
        return resp;
    }
    // Se a UI passou subject/body draft, renderiza isso. Senão, lê do DB.
    let (subject_tpl, body_tpl) = if body.subject.is_some() || body.body.is_some() {
        // Precisamos dos defaults pra saber o que renderizar quando o campo é None.
        match sqlx::query_as::<_, TemplateRow>(
            "SELECT key, label, subject, body, default_subject, default_body, variables, updated_at
               FROM email_template WHERE key = $1",
        )
        .bind(&key)
        .fetch_optional(&state.db)
        .await
        {
            Ok(Some(row)) => (
                body.subject.unwrap_or(row.subject),
                body.body.unwrap_or(row.body),
            ),
            _ => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(ApiResponse::<()>::fail("not_found", "Template não existe.")),
                )
                    .into_response();
            }
        }
    } else {
        match sqlx::query_as::<_, TemplateRow>(
            "SELECT key, label, subject, body, default_subject, default_body, variables, updated_at
               FROM email_template WHERE key = $1",
        )
        .bind(&key)
        .fetch_optional(&state.db)
        .await
        {
            Ok(Some(row)) => (
                if row.subject.trim().is_empty() { row.default_subject } else { row.subject },
                if row.body.trim().is_empty() { row.default_body } else { row.body },
            ),
            _ => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(ApiResponse::<()>::fail("not_found", "Template não existe.")),
                )
                    .into_response();
            }
        }
    };
    let ctx: HashMap<&str, String> = body
        .context
        .iter()
        .map(|(k, v)| (k.as_str(), v.clone()))
        .collect();
    (
        StatusCode::OK,
        Json(ApiResponse::ok(PreviewResult {
            subject: substitute(&subject_tpl, &ctx),
            body: substitute(&body_tpl, &ctx),
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitute_basic() {
        let mut vars = HashMap::new();
        vars.insert("name", "Ana".to_owned());
        vars.insert("proposta", "Ciclovia".to_owned());
        assert_eq!(
            substitute("Olá {{name}}, sua proposta {{proposta}} chegou.", &vars),
            "Olá Ana, sua proposta Ciclovia chegou."
        );
    }

    #[test]
    fn substitute_unknown_stays_literal() {
        let vars: HashMap<&str, String> = HashMap::new();
        assert_eq!(substitute("Oi {{foo}}", &vars), "Oi {{foo}}");
    }

    #[test]
    fn substitute_ignores_singles() {
        let mut vars = HashMap::new();
        vars.insert("x", "1".to_owned());
        assert_eq!(substitute("{ oi } {{x}}", &vars), "{ oi } 1");
    }

    #[test]
    fn substitute_trims_spaces_in_key() {
        let mut vars = HashMap::new();
        vars.insert("k", "V".to_owned());
        assert_eq!(substitute("{{ k }}", &vars), "V");
    }
}

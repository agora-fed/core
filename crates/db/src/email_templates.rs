//! Renderização dos templates de e-mail editáveis (`email_template`,
//! migrations 0151 + 0524).
//!
//! Mora no Tier-0 porque tanto o gateway (proposal_delivery, civic_notify,
//! worker) quanto `dsoc-auth` (signup_verify, password_reset, mandate_invite)
//! enviam e-mail — e o crate de auth não pode depender do gateway. O CRUD
//! admin (`/admin/email-templates`) continua no gateway; aqui é só leitura.
//!
//! Sintaxe do template: apenas `{{var}}`. Sem loops, sem if, sem escape HTML
//! (todos os e-mails são text/plain). Placeholder desconhecido fica literal
//! `{{foo}}` na saída — sinaliza pro admin que a variável está errada.

use sqlx::PgPool;
use std::collections::HashMap;

/// Busca o template `key` no DB e renderiza subject/body com o contexto.
/// Retorna `(subject_final, body_final)` ou `None` se a chave não existe ou
/// o lookup falhou (o caller cai no fallback hardcoded — e-mail nunca deixa
/// de sair porque o DB piscou).
pub async fn render(
    db: &PgPool,
    key: &str,
    vars: &HashMap<&str, String>,
) -> Option<(String, String)> {
    let row: (String, String, String, String) = match sqlx::query_as(
        r"SELECT subject, body, default_subject, default_body
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
    let (subject, body, default_subject, default_body) = row;
    let subject = if subject.trim().is_empty() {
        default_subject
    } else {
        subject
    };
    let body = if body.trim().is_empty() {
        default_body
    } else {
        body
    };
    Some((substitute(&subject, vars), substitute(&body, vars)))
}

/// Substitui `{{var_name}}` no texto pelo valor no HashMap. Variáveis não
/// encontradas ficam literais `{{var_name}}`.
pub fn substitute(input: &str, vars: &HashMap<&str, String>) -> String {
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

//! Rendering of the editable e-mail templates (`email_template`,
//! migrations 0151 + 0524).
//!
//! Mora no Tier-0 porque tanto o gateway (proposal_delivery, civic_notify,
//! worker) quanto `dsoc-auth` (signup_verify, password_reset, mandate_invite)
//! send e-mail — and the auth crate cannot depend on the gateway. The admin
//! CRUD (`/admin/email-templates`) stays in the gateway; this is read-only.
//!
//! Sintaxe do template: apenas `{{var}}`. Sem loops, sem if, sem escape HTML
//! (every e-mail is text/plain). An unknown placeholder stays literal as
//! `{{foo}}` in the output — it signals to the admin that the variable is wrong.

use sqlx::PgPool;
use std::collections::HashMap;

/// Busca o template `key` no DB e renderiza subject/body com o contexto.
/// Returns `(final_subject, final_body)`, or `None` when the key does not exist or
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

/// Replaces `{{var_name}}` in the text with the HashMap value. Variables not
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
                // skip through `}}` inclusive
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
// Presentation layer (0.32.1): templates remain PLAIN TEXT — the admin does not
// precisa saber HTML — e o wrapper embrulha o corpo renderizado num layout
// de marca (logo + cores + links estilizados). Os senders mandam
// multipart/alternative: plain text as the fallback, HTML as the presentation.
// ---------------------------------------------------------------------------

/// Escapa entidades HTML.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Converte o corpo texto-plano em HTML: escapa, transforma URLs `http(s)://`
/// into styled links and newlines into `<br>`. URL-end heuristic: whitespace or a
/// line break ends it; common trailing punctuation (`.,;:!?)`) stays outside the link.
fn body_to_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push_str("<br>\n");
        }
        render_inline(line, &mut out);
    }
    out
}

/// Render one line: escape the text, turn `[label](url)` into a link showing the
/// label, and bare http(s) URLs into a link showing the URL itself.
fn render_inline(line: &str, out: &mut String) {
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut plain_start = 0;
    while i < line.len() {
        // 1) Markdown link [label](url)
        if bytes[i] == b'[' {
            if let Some((label, url, consumed)) = parse_md_link(&line[i..]) {
                out.push_str(&esc(&line[plain_start..i]));
                out.push_str(&format!(
                    "<a href=\"{}\" style=\"color:#15803d;font-weight:600;\">{}</a>",
                    esc(url),
                    esc(label)
                ));
                i += consumed;
                plain_start = i;
                continue;
            }
        }
        // 2) Bare http(s):// URL — the link shows the URL itself.
        let tail = &line[i..];
        if tail.starts_with("http://") || tail.starts_with("https://") {
            out.push_str(&esc(&line[plain_start..i]));
            let end = tail.find(|c: char| c.is_whitespace()).unwrap_or(tail.len());
            let mut url = &tail[..end];
            while let Some(last) = url.chars().last() {
                if matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | ')') {
                    url = &url[..url.len() - last.len_utf8()];
                } else {
                    break;
                }
            }
            out.push_str(&format!(
                "<a href=\"{0}\" style=\"color:#15803d;font-weight:600;\">{0}</a>",
                esc(url)
            ));
            i += url.len();
            plain_start = i;
            continue;
        }
        // Advance one UTF-8 char.
        i += tail.chars().next().map_or(1, char::len_utf8);
    }
    out.push_str(&esc(&line[plain_start..]));
}

/// `[label](http…url)` → `(label, url, bytes_consumed)`. `None` when malformed or
/// when the URL is not http(s) — so `[x](foo)` stays literal (backward-compatible).
fn parse_md_link(s: &str) -> Option<(&str, &str, usize)> {
    let close = s.find("](")?;
    let label = &s[1..close];
    if label.is_empty() || label.contains('[') || label.contains(']') {
        return None;
    }
    let after = &s[close + 2..];
    let end_paren = after.find(')')?;
    let url = &after[..end_paren];
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return None;
    }
    Some((label, url, close + 2 + end_paren + 1))
}

/// Plain-text version of the body: `[label](url)` → `label (url)`; the rest untouched.
/// Used in the multipart `text/plain` part so raw markdown syntax never leaks.
pub fn body_to_plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find('[') {
        if let Some((label, url, consumed)) = parse_md_link(&rest[pos..]) {
            out.push_str(&rest[..pos]);
            out.push_str(label);
            out.push_str(" (");
            out.push_str(url);
            out.push(')');
            rest = &rest[pos + consumed..];
        } else {
            out.push_str(&rest[..=pos]);
            rest = &rest[pos + 1..];
        }
    }
    out.push_str(rest);
    out
}

/// Wrap the body (already-rendered plain text) in the brand e-mail layout:
/// header with the logo, white card, institutional footer. HTML from
/// e-mail conservador — tabelas + estilos inline, sem CSS externo — pra
/// renderizar igual em Gmail/Outlook/Apple Mail.
pub fn html_wrap(body_text: &str) -> String {
    let body_html = body_to_html(body_text);
    format!(
        r#"<!DOCTYPE html>
<html lang="pt-BR">
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"></head>
<body style="margin:0;padding:0;background-color:#f1f5f9;">
<table role="presentation" width="100%" cellpadding="0" cellspacing="0" style="background-color:#f1f5f9;padding:24px 12px;">
<tr><td align="center">
<table role="presentation" width="600" cellpadding="0" cellspacing="0" style="max-width:600px;width:100%;">
  <tr><td style="padding:8px 8px 16px;" align="left">
    <table role="presentation" cellpadding="0" cellspacing="0"><tr>
      <td><img src="https://democracia.social.br/favicon-512.png" width="40" height="40" alt="" style="display:block;border:0;"></td>
      <td style="padding-left:10px;font-family:-apple-system,Segoe UI,Roboto,Helvetica,Arial,sans-serif;font-size:22px;font-weight:700;">
        <span style="color:#1e3a8a;">Democracia</span><span style="color:#15803d;">BR</span>
      </td>
    </tr></table>
  </td></tr>
  <tr><td style="background-color:#ffffff;border-radius:12px;border-top:4px solid #15803d;padding:32px 32px 28px;font-family:-apple-system,Segoe UI,Roboto,Helvetica,Arial,sans-serif;font-size:16px;line-height:1.6;color:#0f172a;">
{body_html}
  </td></tr>
  <tr><td style="padding:20px 8px;font-family:-apple-system,Segoe UI,Roboto,Helvetica,Arial,sans-serif;font-size:13px;line-height:1.5;color:#64748b;" align="center">
    <a href="https://democracia.social.br" style="color:#15803d;font-weight:600;text-decoration:none;">democracia.social.br</a>
    — infraestrutura pública de accountability.<br>
    O silêncio também é registro público.
  </td></tr>
</table>
</td></tr>
</table>
</body>
</html>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_wrap_escapes_and_linkifies() {
        let html = html_wrap("Olá <você>,\nAcesse: https://democracia.social.br/x. Fim");
        assert!(html.contains("Olá &lt;você&gt;,<br>"));
        assert!(html.contains(
            "<a href=\"https://democracia.social.br/x\" style=\"color:#15803d;font-weight:600;\">https://democracia.social.br/x</a>."
        ));
        assert!(html.contains("favicon-512.png"));
    }

    #[test]
    fn body_to_html_url_at_end_of_line() {
        let html = body_to_html("Link:\nhttps://x.dev/a?b=c\ntchau");
        assert!(html.contains("<a href=\"https://x.dev/a?b=c\""));
        assert!(html.ends_with("tchau"));
    }

    #[test]
    fn body_to_html_markdown_link_name_becomes_anchor() {
        // The name becomes the link; surrounding text (parentheses included) stays intact.
        let html = body_to_html(
            "Perfil: [Enfermeira Nelci](https://democracia.social.br/politicos/?id=abc) (PDT/SP — Vereador(a)).",
        );
        assert!(html.contains(
            "<a href=\"https://democracia.social.br/politicos/?id=abc\" style=\"color:#15803d;font-weight:600;\">Enfermeira Nelci</a>"
        ));
        assert!(html.contains("(PDT/SP — Vereador(a))."));
        // A label containing < is escaped.
        let evil = body_to_html("[<b>x</b>](https://a.b)");
        assert!(evil.contains(">&lt;b&gt;x&lt;/b&gt;</a>"));
    }

    #[test]
    fn body_to_plain_unwraps_markdown_links() {
        assert_eq!(
            body_to_plain("Oi [Nelci](https://a.b/c) tudo bem [x](nao-url)?"),
            "Oi Nelci (https://a.b/c) tudo bem [x](nao-url)?"
        );
    }

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

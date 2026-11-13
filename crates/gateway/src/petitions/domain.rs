//! # Pure rules of public petitions: input checks, usernames and link tokens.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use rand::RngCore;
use sha2::{Digest, Sha256};

pub(crate) const TITLE_MAX: usize = 160;
pub(crate) const BODY_MAX: usize = 20_000;
pub(crate) const NAME_MIN: usize = 2;
pub(crate) const NAME_MAX: usize = 80;
pub(crate) const CITY_MAX: usize = 80;
pub(crate) const EMAIL_MAX: usize = 254;
pub(crate) const THRESHOLD_MAX: i32 = 10_000_000;
const USERNAME_STEM_MAX: usize = 20;

/// A checked petition draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Draft {
    pub title: String,
    pub body: String,
    pub threshold: i32,
}

/// Does the text contain something a mail client or our layout would turn into a link?
fn looks_like_link(text: &str) -> bool {
    let t = text.to_lowercase();
    t.contains("://")
        || t.contains("www.")
        || t.contains("](")
        || t.contains('@')
        || t.split_whitespace().any(|w| {
            let w = w.trim_matches(|c: char| !c.is_alphanumeric());
            w.contains('.')
                && w.rsplit('.').next().is_some_and(|tld| {
                    tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic())
                })
        })
}

/// Check a new petition. Returns the Portuguese message to show on failure.
pub(crate) fn check_draft(title: &str, body: &str, threshold: i32) -> Result<Draft, &'static str> {
    let title = title.trim();
    let body = body.trim();
    if title.chars().count() < 5 || title.chars().count() > TITLE_MAX {
        return Err("O título precisa ter entre 5 e 160 caracteres.");
    }
    // The title goes into confirmation e-mails sent from this server's address, and the
    // mail layout turns URLs and [text](url) into links. Refuse both, so a petition can
    // never turn the official sender into a phishing channel.
    if title.chars().any(char::is_control) || looks_like_link(title) {
        return Err("O título não pode ter links ou endereços de sites.");
    }
    if body.chars().count() < 20 || body.chars().count() > BODY_MAX {
        return Err("O texto precisa ter entre 20 e 20.000 caracteres.");
    }
    if !(1..=THRESHOLD_MAX).contains(&threshold) {
        return Err("A meta de assinaturas precisa ser um número entre 1 e 10.000.000.");
    }
    Ok(Draft {
        title: title.to_owned(),
        body: body.to_owned(),
        threshold,
    })
}

/// A checked public signer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Signer {
    pub name: String,
    pub email: String,
    pub city: Option<String>,
}

fn has_control(s: &str) -> bool {
    s.chars().any(char::is_control)
}

/// Check a lower-cased address. Simple on purpose: the confirmation proves it works.
pub(crate) fn normalize_email(email: &str) -> Option<String> {
    let email = email.trim().to_lowercase();
    let (local, domain) = email.split_once('@')?;
    let ok = email.len() <= EMAIL_MAX
        && !local.is_empty()
        && local.len() <= 64
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !email.contains(char::is_whitespace)
        && !domain.contains('@')
        && !has_control(&email);
    ok.then_some(email)
}

/// Letters (any script), spaces and `.'-`. The name goes into outgoing e-mail, so it
/// must not be able to carry a link, markup or an address.
fn name_char(c: char) -> bool {
    c.is_alphabetic() || c == ' ' || matches!(c, '.' | '\'' | '-' | '’')
}

/// Check what an anonymous signer typed.
pub(crate) fn check_signer(
    name: &str,
    email: &str,
    city: Option<&str>,
) -> Result<Signer, &'static str> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = name.chars().count();
    if !(NAME_MIN..=NAME_MAX).contains(&count) || !name.chars().all(name_char) {
        return Err("Informe seu nome (de 2 a 80 letras; sem números ou links).");
    }
    let email = normalize_email(email).ok_or("Informe um e-mail válido.")?;
    let city = city
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_owned);
    if city
        .as_deref()
        .is_some_and(|c| c.chars().count() > CITY_MAX || has_control(c))
    {
        return Err("A cidade pode ter até 80 caracteres.");
    }
    Ok(Signer { name, email, city })
}

/// Fold common Latin accents to ASCII. Other characters drop out.
fn fold(c: char) -> Option<char> {
    let folded = match c {
        'á' | 'à' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'ç' => 'c',
        'ñ' => 'n',
        c if c.is_ascii_alphanumeric() => c,
        _ => return None,
    };
    Some(folded)
}

/// The readable part of a username, from the signer's name: `Maria da Silva` → `maria_da_silva`.
/// Always starts with a letter and has 3 to 20 characters.
pub(crate) fn username_stem(name: &str) -> String {
    let words: Vec<String> = name
        .to_lowercase()
        .split_whitespace()
        .map(|w| w.chars().filter_map(fold).collect::<String>())
        .filter(|w| !w.is_empty())
        .collect();
    let mut stem = words.join("_");
    if !stem.starts_with(|c: char| c.is_ascii_lowercase()) {
        stem = format!("p{stem}");
    }
    if stem.len() < 3 {
        stem = "pessoa".to_owned();
    }
    stem.truncate(USERNAME_STEM_MAX);
    stem.trim_end_matches('_').to_owned()
}

/// A username candidate: the stem plus four digits, for example `maria_silva_4821`.
pub(crate) fn username_candidate(stem: &str, digits: u16) -> String {
    format!("{stem}_{:04}", digits % 10_000)
}

/// A fresh single-use link token (URL-safe) and the hash stored in the database.
pub(crate) fn new_token() -> (String, Vec<u8>) {
    let mut raw = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    let token = URL_SAFE_NO_PAD.encode(raw);
    let hash = token_hash(&token);
    (token, hash)
}

/// SHA-256 of a link token. Only the hash is stored, so a database leak gives no links.
pub(crate) fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// Shorten a body for list views.
pub(crate) fn summary(body: &str, max: usize) -> String {
    if body.chars().count() <= max {
        return body.to_owned();
    }
    let cut: String = body.chars().take(max).collect();
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn drafts_are_trimmed_and_bounded() {
        let d = check_draft("  Mais ciclovias  ", &"x".repeat(30), 100).unwrap();
        assert_eq!(d.title, "Mais ciclovias");
        assert!(check_draft("abc", &"x".repeat(30), 100).is_err());
        assert!(check_draft("Título bom", "curto", 100).is_err());
        assert!(check_draft("Título bom", &"x".repeat(30), 0).is_err());
        assert!(check_draft("Título bom", &"x".repeat(30), THRESHOLD_MAX + 1).is_err());
        for bad in [
            "[Confirme](https://evil.example)",
            "Veja www.evil.example agora",
            "Acesse evil.com já",
            "Linha\nquebrada aqui",
            "Fale com a@b.org",
        ] {
            assert!(check_draft(bad, &"x".repeat(30), 10).is_err(), "{bad}");
        }
        assert!(check_draft("Mais 2.5 km de ciclovia", &"x".repeat(30), 10).is_ok());
    }

    #[test]
    fn emails_are_normalized_or_refused() {
        assert_eq!(
            normalize_email(" Ana@Example.ORG "),
            Some("ana@example.org".into())
        );
        for bad in [
            "",
            "ana",
            "ana@",
            "@x.org",
            "ana@x",
            "a b@x.org",
            "a@x.org.",
            "a@@x.org",
        ] {
            assert_eq!(normalize_email(bad), None, "{bad}");
        }
        assert_eq!(normalize_email(&format!("{}@x.org", "a".repeat(65))), None);
    }

    #[test]
    fn signers_are_checked() {
        let s = check_signer("  Ana   Maria ", "ana@x.org", Some("  ")).unwrap();
        assert_eq!(s.name, "Ana Maria");
        assert_eq!(s.city, None);
        let s = check_signer("Ana", "ana@x.org", Some(" Recife ")).unwrap();
        assert_eq!(s.city.as_deref(), Some("Recife"));
        assert!(check_signer("A", "ana@x.org", None).is_err());
        assert!(check_signer("Ana", "nope", None).is_err());
        assert!(check_signer("Ana", "ana@x.org", Some(&"r".repeat(81))).is_err());
        assert!(check_signer("Ana\u{0}", "ana@x.org", None).is_err());
        assert!(check_signer("Ana https://x.io", "ana@x.org", None).is_err());
        assert!(check_signer("<b>Ana</b>", "ana@x.org", None).is_err());
        assert!(check_signer("Maria D'Ávila-Souza Jr.", "ana@x.org", None).is_ok());
        assert!(check_signer("李小龍", "ana@x.org", None).is_ok());
    }

    #[test]
    fn usernames_come_from_the_name() {
        assert_eq!(username_stem("Maria da Conceição"), "maria_da_conceicao");
        assert_eq!(username_stem("José Ñandú"), "jose_nandu");
        assert_eq!(username_stem("2Pac"), "p2pac");
        assert_eq!(username_stem("李"), "pessoa");
        assert_eq!(username_stem("Ab"), "pessoa");
        let long = username_stem("Alessandra Cristina Figueiredo");
        assert!(long.len() <= 20 && !long.ends_with('_'));
        assert_eq!(username_candidate("ana", 7), "ana_0007");
        assert_eq!(username_candidate("ana", 12_345), "ana_2345");
    }

    #[test]
    fn tokens_hash_consistently() {
        let (token, hash) = new_token();
        assert_eq!(token_hash(&token), hash);
        assert_eq!(hash.len(), 32);
        assert_ne!(new_token().0, token);
    }

    #[test]
    fn summaries_cut_on_characters() {
        assert_eq!(summary("curto", 10), "curto");
        assert_eq!(summary("ação ação ação", 4), "ação…");
    }
}

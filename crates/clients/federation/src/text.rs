//! # Text extraction — hashtags and mentions from Note content.
//!
//! Pure functions, no allocations beyond the returned `Vec`s, no regex crate
//! (kept out of the workspace dep tree). Used by both the outbound publisher
//! (extracts from user-authored content before persisting) and the inbound
//! receive path (extracts from remote HTML after sanitization) so mentions
//! and hashtags are indexed identically regardless of origin.
//!
//! ## Rules
//!
//! **Hashtag** — matches `#` followed by ≥1 of `[A-Za-z0-9_]` (ASCII) OR any
//! non-ASCII letter/digit (relies on `char::is_alphanumeric` which is
//! Unicode-aware in Rust). The `#` MUST NOT be preceded by an alphanumeric
//! char (so `abc#def` does not match — Mastodon's rule).
//!
//! **Mention** — matches `@user` optionally followed by `@host.tld`. The
//! leading `@` MUST NOT be preceded by an alphanumeric (so `user@example.com`
//! e-mail-shaped text does not match). `user` = `[A-Za-z0-9_.-]+`, `host` =
//! at least one dot-separated segment of `[A-Za-z0-9-]+`.
//!
//! ## Normalization
//!
//! Hashtag `tag_normalized` = lowercase + NFD-decompose + drop combining
//! marks (so `#SaúdePública` → `saudepublica`). Requires the `unicode-normalization`
//! feature only if we grow one; today we do the ASCII-fold subset that
//! covers Latin-script diacritics inline — enough for
//! BR use, expandable later.
//!
//! Mention: no normalization; the handle is preserved as-authored (case-
//! sensitive is Mastodon's default too).

use std::collections::HashSet;

/// A hashtag as extracted from source text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hashtag {
    /// The tag as it appeared in the source (without the leading `#`).
    pub original: String,
    /// Normalized lookup key (lowercase, ASCII-folded).
    pub normalized: String,
}

/// A mention as extracted from source text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mention {
    /// The user part (before any `@host`).
    pub user: String,
    /// The host part, or `None` for a local mention (`@alice` alone).
    pub host: Option<String>,
    /// The joined display handle: `alice` (local) or `alice@remote.tld` (remote).
    pub handle: String,
}

impl Mention {
    /// Best-effort actor URL. Requires knowing our own `public_origin` to
    /// resolve a local mention. For remote mentions we can only guess an
    /// `https://{host}/users/{user}` shape (the Mastodon convention); the
    /// authoritative URL comes from a WebFinger lookup — see [`ResolvedMention`],
    /// which the gateway produces at publish time; this guess is the fallback
    /// when the remote instance is unreachable.
    #[must_use]
    pub fn best_actor_url(&self, public_origin: &str) -> String {
        match &self.host {
            None => format!(
                "{}/actors/{}",
                public_origin.trim_end_matches('/'),
                self.user
            ),
            Some(h) => format!("https://{h}/users/{}", self.user),
        }
    }
}

/// A mention resolved to its authoritative actor via WebFinger + actor fetch.
///
/// Produced by the gateway (the only layer with network access) before a Note is
/// persisted, so the service layer can put the real actor id in the `tag[]` href,
/// address the actor in `cc`, and queue a delivery to its inbox. A mention that
/// failed to resolve simply has no entry and falls back to
/// [`Mention::best_actor_url`] — the pre-resolution behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMention {
    /// The display handle as authored: `alice` (local) or `alice@remote.tld`.
    pub handle: String,
    /// The authoritative actor id URL (the WebFinger `self` link's target).
    pub actor_url: String,
    /// The actor's personal inbox — target for direct delivery, when exposed.
    pub inbox_url: Option<String>,
}

/// Extract all hashtags from a source text, deduped by `normalized` in
/// first-seen order.
#[must_use]
pub fn extract_hashtags(text: &str) -> Vec<Hashtag> {
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        // Find the next '#'.
        let Some(rel) = text[i..].find('#') else {
            break;
        };
        let hash_idx = i + rel;
        // Reject when the char BEFORE '#' is alphanumeric (avoids "abc#def").
        if hash_idx > 0 {
            let prev = text[..hash_idx].chars().next_back();
            if let Some(p) = prev {
                if p.is_alphanumeric() {
                    i = hash_idx + 1;
                    continue;
                }
            }
        }
        // Collect the tag body: alphanumeric or '_'.
        let mut end = hash_idx + 1;
        for ch in text[hash_idx + 1..].chars() {
            if is_tag_char(ch) {
                end += ch.len_utf8();
            } else {
                break;
            }
        }
        if end > hash_idx + 1 {
            let original = text[hash_idx + 1..end].to_owned();
            let normalized = normalize_tag(&original);
            if !normalized.is_empty() && seen.insert(normalized.clone()) {
                out.push(Hashtag {
                    original,
                    normalized,
                });
            }
        }
        i = end.max(hash_idx + 1);
    }
    out
}

/// Extract all mentions from a source text, deduped by `handle` in first-seen order.
#[must_use]
pub fn extract_mentions(text: &str) -> Vec<Mention> {
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut i = 0usize;
    while i < text.len() {
        let Some(rel) = text[i..].find('@') else {
            break;
        };
        let at_idx = i + rel;
        // Reject when preceded by an alphanumeric or '.' — dodges emails.
        if at_idx > 0 {
            let prev = text[..at_idx].chars().next_back();
            if let Some(p) = prev {
                if p.is_alphanumeric() || p == '.' {
                    i = at_idx + 1;
                    continue;
                }
            }
        }
        // Consume user part.
        let user_start = at_idx + 1;
        let mut user_end = user_start;
        for ch in text[user_start..].chars() {
            if is_user_char(ch) {
                user_end += ch.len_utf8();
            } else {
                break;
            }
        }
        if user_end == user_start {
            i = at_idx + 1;
            continue;
        }
        let user = text[user_start..user_end].to_owned();
        // Optionally an '@host.tld' suffix.
        let (host, end) = if text[user_end..].starts_with('@') {
            let host_start = user_end + 1;
            let mut host_end = host_start;
            for ch in text[host_start..].chars() {
                if is_host_char(ch) {
                    host_end += ch.len_utf8();
                } else {
                    break;
                }
            }
            let host_slice = &text[host_start..host_end];
            if valid_host(host_slice) {
                (Some(host_slice.to_owned()), host_end)
            } else {
                (None, user_end)
            }
        } else {
            (None, user_end)
        };
        let handle = match &host {
            None => user.clone(),
            Some(h) => format!("{user}@{h}"),
        };
        if seen.insert(handle.clone()) {
            out.push(Mention { user, host, handle });
        }
        i = end.max(at_idx + 1);
    }
    out
}

fn is_tag_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_user_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')
}

fn is_host_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '.')
}

/// A domain-shaped host: at least one dot, non-empty labels, no trailing dot.
fn valid_host(s: &str) -> bool {
    if !s.contains('.') || s.starts_with('.') || s.ends_with('.') {
        return false;
    }
    s.split('.').all(|seg| !seg.is_empty() && seg.len() <= 63)
}

/// Lowercase + strip common Latin diacritics used in PT/ES/FR text. Not a full
/// NFD implementation, but enough for BR use today; expand as we grow.
fn normalize_tag(s: &str) -> String {
    let lowered = s.to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    for c in lowered.chars() {
        let mapped: &str = match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' | 'å' => "a",
            'é' | 'è' | 'ê' | 'ë' => "e",
            'í' | 'ì' | 'î' | 'ï' => "i",
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => "o",
            'ú' | 'ù' | 'û' | 'ü' => "u",
            'ý' | 'ÿ' => "y",
            'ç' => "c",
            'ñ' => "n",
            _ => {
                if c.is_alphanumeric() || c == '_' {
                    out.push(c);
                    continue;
                }
                continue;
            }
        };
        out.push_str(mapped);
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn hashtag_basic() {
        let tags = extract_hashtags("democracia e #brasil hoje");
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].original, "brasil");
        assert_eq!(tags[0].normalized, "brasil");
    }

    #[test]
    fn hashtag_dedupe_and_diacritics() {
        let tags = extract_hashtags("#SaúdePública é diferente de #saudepublica? não");
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].normalized, "saudepublica");
    }

    #[test]
    fn hashtag_rejects_midword() {
        // "abc#def" should not match.
        let tags = extract_hashtags("abc#def #real");
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].original, "real");
    }

    #[test]
    fn mention_local() {
        let ms = extract_mentions("olá @alice, tudo bem?");
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].user, "alice");
        assert!(ms[0].host.is_none());
        assert_eq!(ms[0].handle, "alice");
    }

    #[test]
    fn mention_remote() {
        let ms = extract_mentions("cc @bob@mastodon.social ok");
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].user, "bob");
        assert_eq!(ms[0].host.as_deref(), Some("mastodon.social"));
        assert_eq!(ms[0].handle, "bob@mastodon.social");
    }

    #[test]
    fn mention_rejects_email_shape() {
        // "alice@bob.com" without leading space (or after alphanumeric) is an
        // email-shape and MUST NOT be a mention.
        let ms = extract_mentions("write to alice@bob.com please");
        assert!(ms.is_empty());
    }

    #[test]
    fn actor_url_local_and_remote() {
        let m_local = Mention {
            user: "alice".into(),
            host: None,
            handle: "alice".into(),
        };
        assert_eq!(
            m_local.best_actor_url("https://democracia.social.br"),
            "https://democracia.social.br/actors/alice"
        );
        let m_remote = Mention {
            user: "bob".into(),
            host: Some("mastodon.social".into()),
            handle: "bob@mastodon.social".into(),
        };
        assert_eq!(
            m_remote.best_actor_url("https://democracia.social.br"),
            "https://mastodon.social/users/bob"
        );
    }
}

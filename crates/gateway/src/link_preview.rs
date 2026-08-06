//! Link preview cards — the thumbnail a pasted URL deserves (migration 0680).
//!
//! A citizen posts a YouTube link and the note renders as bare text: no title, no
//! image. Mastodon calls this a `preview_card`; ours has been hardcoded to `null`
//! since the API was written, so nothing ever showed.
//!
//! The fetch goes through [`crate::outbound`] (issue #9), which matters more here than
//! anywhere else: the URL comes from whatever a stranger typed into a post, so this is
//! the most directly attacker-controlled fetch on the platform. It runs with redirects
//! ENABLED but revalidated per hop, because `youtu.be/<id>` answers 303 and refusing
//! redirects outright would fail most short links people actually paste.
//!
//! Cached per URL rather than per note: the same link posted a thousand times is
//! fetched once, which is also what stops a repost loop from hammering a third party.
//! Failures are cached too — otherwise a dead link is re-attempted on every render.

use sha2::Digest as _;
use uuid::Uuid;

/// Bytes of a page we are willing to read while looking for `<meta>` tags. The tags
/// live in `<head>`; a video page's body can be megabytes of script we do not want.
const MAX_PREVIEW_BYTES: usize = 512 * 1024;

/// Redirect hops allowed. Two covers `youtu.be` → `youtube.com` and one more; a chain
/// longer than that is a tracker, not a destination.
const MAX_HOPS: u8 = 2;

/// How long a cached card stays fresh.
const TTL_HOURS: i64 = 24 * 7;

/// A preview card, shaped like Mastodon's so the client contract is familiar.
#[derive(Debug, Clone, Default, serde::Serialize, sqlx::FromRow)]
pub struct PreviewCard {
    /// The URL the card describes (after redirects).
    pub url: String,
    /// Page title, or the video title.
    pub title: Option<String>,
    /// Short description, when the page offers one.
    pub description: Option<String>,
    /// Thumbnail. This is the field the whole feature exists for.
    pub image_url: Option<String>,
    /// e.g. `YouTube`.
    pub site_name: Option<String>,
    /// `link` | `video` | `photo`.
    pub kind: String,
}

fn url_hash(url: &str) -> Vec<u8> {
    sha2::Sha256::digest(url.as_bytes()).to_vec()
}

/// The first http(s) URL in a plain-text or HTML note body.
///
/// One card per note, like Mastodon: a post with five links gets the first one, not a
/// wall of cards. Trailing punctuation is trimmed because people write
/// "look at https://x.com/y." and the period is the sentence's, not the URL's.
#[must_use]
pub fn extract_first_url(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let start = lower.find("https://").or_else(|| lower.find("http://"))?;
    let rest = &text[start..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '<' || c == '"' || c == '\'')
        .unwrap_or(rest.len());
    let mut url = &rest[..end];
    // Strip trailing sentence punctuation and unbalanced closing brackets.
    while let Some(last) = url.chars().last() {
        if matches!(
            last,
            '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}' | '"' | '\''
        ) {
            url = &url[..url.len() - last.len_utf8()];
        } else {
            break;
        }
    }
    (url.len() > 10).then(|| url.to_owned())
}

/// Read the content of a `<meta>` tag by `property` or `name`.
///
/// A deliberate small scan rather than a full HTML parse: the input is hostile and
/// unbounded, and everything needed lives in a handful of self-closing tags. Handles
/// both attribute orders (`property` before or after `content`) and both quote styles,
/// which is where naive versions of this usually break.
#[must_use]
pub fn meta_content(html: &str, key: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let needles = [
        format!("property=\"{key}\""),
        format!("property='{key}'"),
        format!("name=\"{key}\""),
        format!("name='{key}'"),
    ];
    for needle in &needles {
        let mut from = 0usize;
        while let Some(pos) = lower[from..].find(needle.as_str()) {
            let abs = from + pos;
            // The enclosing tag: back to '<', forward to '>'.
            let tag_start = lower[..abs].rfind('<')?;
            let tag_end = lower[abs..].find('>').map(|e| abs + e)?;
            let tag = &html[tag_start..tag_end];
            if let Some(value) = attr_value(tag, "content") {
                if !value.trim().is_empty() {
                    return Some(decode_entities(value.trim()));
                }
            }
            from = tag_end;
        }
    }
    None
}

/// Value of `attr` inside a single tag, for either quote style.
fn attr_value(tag: &str, attr: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let at = lower.find(&format!("{attr}="))?;
    let after = &tag[at + attr.len() + 1..];
    let quote = after.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let body = &after[1..];
    let end = body.find(quote)?;
    Some(body[..end].to_owned())
}

/// The handful of entities that actually show up in title/description text.
fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

/// Build a card from a fetched page. `final_url` is the URL after redirects.
#[must_use]
pub fn card_from_html(html: &str, final_url: &str) -> PreviewCard {
    let title = meta_content(html, "og:title")
        .or_else(|| meta_content(html, "twitter:title"))
        .or_else(|| {
            // Plain `<title>` as the last resort — many pages carry no OG at all.
            let lower = html.to_ascii_lowercase();
            let start = lower.find("<title")?;
            let open_end = lower[start..].find('>').map(|e| start + e + 1)?;
            let close = lower[open_end..].find("</title>").map(|e| open_end + e)?;
            let raw = html[open_end..close].trim();
            (!raw.is_empty()).then(|| decode_entities(raw))
        });
    let kind = match meta_content(html, "og:type").as_deref() {
        Some(t) if t.contains("video") => "video",
        Some("photo" | "image") => "photo",
        _ => "link",
    };
    PreviewCard {
        url: meta_content(html, "og:url").unwrap_or_else(|| final_url.to_owned()),
        title,
        description: meta_content(html, "og:description")
            .or_else(|| meta_content(html, "description")),
        image_url: meta_content(html, "og:image").or_else(|| meta_content(html, "twitter:image")),
        site_name: meta_content(html, "og:site_name"),
        kind: kind.to_owned(),
    }
}

/// A card is only worth showing if it has something to show.
#[must_use]
pub fn is_useful(card: &PreviewCard) -> bool {
    card.title.is_some() || card.image_url.is_some()
}

/// The oEmbed endpoint for providers whose HTML is not worth scraping.
///
/// YouTube is the reason this exists. Its watch page puts a megabyte of inline script
/// BEFORE the OpenGraph tags — measured 2026-08-06: `og:image` is not within the first
/// 512 KiB — so scraping either finds nothing or forces us to download the whole page
/// for four tags. The oEmbed endpoint answers the same question in ~400 bytes, with no
/// redirect, and it is the documented interface rather than a scrape.
#[must_use]
pub fn oembed_endpoint(url: &str) -> Option<String> {
    let lower = url.to_ascii_lowercase();
    let host_is = |h: &str| {
        lower.starts_with(&format!("https://{h}/"))
            || lower.starts_with(&format!("https://www.{h}/"))
            || lower.starts_with(&format!("https://m.{h}/"))
    };
    if host_is("youtube.com") || host_is("youtu.be") {
        return Some(format!(
            "https://www.youtube.com/oembed?url={}&format=json",
            urlencode(url)
        ));
    }
    if host_is("vimeo.com") {
        return Some(format!(
            "https://vimeo.com/api/oembed.json?url={}",
            urlencode(url)
        ));
    }
    None
}

/// Percent-encode the characters that matter inside a query parameter.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(b));
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Build a card from an oEmbed JSON document.
#[must_use]
pub fn card_from_oembed(json: &serde_json::Value, source_url: &str) -> PreviewCard {
    let str_of = |k: &str| {
        json.get(k)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    let kind = match json.get("type").and_then(serde_json::Value::as_str) {
        Some("video") => "video",
        Some("photo") => "photo",
        _ => "link",
    };
    PreviewCard {
        url: source_url.to_owned(),
        title: str_of("title"),
        // oEmbed has no description field; the author is the useful second line.
        description: str_of("author_name"),
        image_url: str_of("thumbnail_url"),
        site_name: str_of("provider_name"),
        kind: kind.to_owned(),
    }
}

/// Policy for preview fetches: HTTPS, revalidated redirects, small body.
fn policy() -> crate::outbound::OutboundPolicy {
    crate::outbound::OutboundPolicy {
        max_body: MAX_PREVIEW_BYTES,
        max_redirects: MAX_HOPS,
        // Truncate rather than refuse: YouTube's watch page is well over the cap, and
        // everything this feature needs is in the first few KB of <head>.
        truncate_oversized: true,
        timeout: std::time::Duration::from_secs(8),
        ..Default::default()
    }
}

/// Fetch (or read from cache) the card for `url`, and attach it to `object_uri`.
///
/// Best-effort throughout: this runs in the background after a note is posted, and a
/// failure must never affect whether the note itself exists.
pub async fn resolve_and_attach(db: &sqlx::PgPool, object_uri: &str, url: &str) {
    let hash = url_hash(url);

    let fresh: Option<(bool,)> = sqlx::query_as(
        "SELECT ok FROM link_preview \
          WHERE url_hash = $1 AND fetched_at > now() - make_interval(hours => $2::int)",
    )
    .bind(&hash)
    .bind(i32::try_from(TTL_HOURS).unwrap_or(168))
    .fetch_optional(db)
    .await
    .unwrap_or(None);

    if fresh.is_none() {
        let card = fetch_card(url).await;
        let ok = card.as_ref().is_some_and(is_useful);
        let card = card.unwrap_or_default();
        // Negative results are stored too — see the migration's note on why.
        let _ = sqlx::query(
            "INSERT INTO link_preview \
               (url_hash, url, ok, title, description, image_url, site_name, kind, fetched_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now()) \
             ON CONFLICT (url_hash) DO UPDATE SET \
               ok = EXCLUDED.ok, title = EXCLUDED.title, description = EXCLUDED.description, \
               image_url = EXCLUDED.image_url, site_name = EXCLUDED.site_name, \
               kind = EXCLUDED.kind, fetched_at = now()",
        )
        .bind(&hash)
        .bind(url)
        .bind(ok)
        .bind(&card.title)
        .bind(&card.description)
        .bind(&card.image_url)
        .bind(&card.site_name)
        .bind(if card.kind.is_empty() {
            "link".to_owned()
        } else {
            card.kind.clone()
        })
        .execute(db)
        .await;
    }

    let _ = sqlx::query(
        "INSERT INTO note_link_preview (object_uri, url_hash) VALUES ($1, $2) \
         ON CONFLICT (object_uri) DO UPDATE SET url_hash = EXCLUDED.url_hash",
    )
    .bind(object_uri)
    .bind(&hash)
    .execute(db)
    .await;
}

/// Fetch and parse one URL. `None` = nothing usable.
async fn fetch_card(url: &str) -> Option<PreviewCard> {
    // oEmbed first where it exists — see `oembed_endpoint` for why scraping YouTube
    // does not work.
    if let Some(endpoint) = oembed_endpoint(url) {
        let json_policy = crate::outbound::OutboundPolicy {
            max_body: 64 * 1024,
            max_redirects: MAX_HOPS,
            timeout: std::time::Duration::from_secs(8),
            ..Default::default()
        };
        let hdrs = vec![("accept".to_owned(), "application/json".to_owned())];
        match crate::outbound::guarded_get_following(&endpoint, &hdrs, &json_policy).await {
            Ok(bytes) => {
                if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                    let card = card_from_oembed(&json, url);
                    if is_useful(&card) {
                        return Some(card);
                    }
                }
            }
            Err(err) => {
                // Fall through to the generic scrape rather than giving up.
                tracing::info!(url, error = %err, "oEmbed fetch failed; trying OpenGraph");
            }
        }
    }

    let headers = vec![(
        "accept".to_owned(),
        "text/html,application/xhtml+xml".to_owned(),
    )];
    match crate::outbound::guarded_get_following(url, &headers, &policy()).await {
        Ok(bytes) => {
            // Lossy on purpose: a page in a legacy encoding should degrade to a
            // slightly mangled title, not to no card at all.
            let html = String::from_utf8_lossy(&bytes);
            let card = card_from_html(&html, url);
            is_useful(&card).then_some(card)
        }
        Err(err) => {
            tracing::info!(url, error = %err, "link preview fetch failed");
            None
        }
    }
}

/// Read the stored card for a note, if it has a usable one.
pub async fn card_for(db: &sqlx::PgPool, object_uri: &str) -> Option<PreviewCard> {
    sqlx::query_as::<_, PreviewCard>(
        "SELECT p.url, p.title, p.description, p.image_url, p.site_name, p.kind \
           FROM note_link_preview n JOIN link_preview p ON p.url_hash = n.url_hash \
          WHERE n.object_uri = $1 AND p.ok = true",
    )
    .bind(object_uri)
    .fetch_optional(db)
    .await
    .unwrap_or(None)
}

/// Kick off a preview resolution in the background for a freshly-stored note.
///
/// Spawned rather than awaited: posting must not wait on a third party's server, and
/// the card appearing a second later is the correct trade.
pub fn spawn_for_note(db: &sqlx::PgPool, object_uri: &str, content: &str) {
    let Some(url) = extract_first_url(content) else {
        return;
    };
    let db = db.clone();
    let object_uri = object_uri.to_owned();
    tokio::spawn(async move {
        resolve_and_attach(&db, &object_uri, &url).await;
    });
}

/// Resolve cards for notes that predate this feature, a bounded batch at a time.
///
/// Without this, every note already in the database stays card-less forever: the
/// resolution only fires when a note is STORED, and those were stored before the
/// feature existed. The reported case — a YouTube link posted before today — is
/// exactly this.
///
/// Reads both sides (local outbox payload and remote timeline HTML), skips anything
/// that already has a card row, and stops at `limit`. Returns how many it attempted.
pub async fn backfill(db: &sqlx::PgPool, limit: i64) -> u64 {
    let rows: Vec<(String, String)> = sqlx::query_as(
        r"SELECT object_uri, content FROM (
              SELECT (o.activity_id) AS object_uri,
                     COALESCE(o.payload #>> '{object,content}', '') AS content
                FROM federation_outbox_entry o
              UNION ALL
              SELECT t.object_uri, t.content_html
                FROM federation_timeline_entry t
          ) n
          WHERE n.content ILIKE '%http%'
            AND NOT EXISTS (SELECT 1 FROM note_link_preview p WHERE p.object_uri = n.object_uri)
          LIMIT $1",
    )
    .bind(limit)
    .fetch_all(db)
    .await
    .unwrap_or_default();

    let mut done = 0u64;
    for (object_uri, content) in rows {
        // The outbox stores the ACTIVITY id; the feed keys cards by the OBJECT id.
        let uri = object_uri.replace("/activities/note-", "/objects/");
        if let Some(url) = extract_first_url(&content) {
            resolve_and_attach(db, &uri, &url).await;
            done += 1;
        }
    }
    done
}

/// Unused today, kept so the id type stays visible to callers that build notes.
#[allow(dead_code)]
fn _type_anchor(_id: Uuid) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_the_first_link_and_trims_sentence_punctuation() {
        assert_eq!(
            extract_first_url("olha isso https://youtu.be/dQw4w9WgXcQ."),
            Some("https://youtu.be/dQw4w9WgXcQ".to_owned())
        );
        assert_eq!(
            extract_first_url("(https://example.com/a)"),
            Some("https://example.com/a".to_owned())
        );
        // One card per note: the FIRST link wins.
        assert_eq!(
            extract_first_url("https://a.example/1 e https://b.example/2"),
            Some("https://a.example/1".to_owned())
        );
        assert_eq!(extract_first_url("sem link nenhum aqui"), None);
        assert_eq!(extract_first_url(""), None);
    }

    #[test]
    fn extracts_from_html_content_too() {
        // Notes arrive as HTML from the composer and from remote instances.
        let html = r#"<p>veja <a href="https://youtu.be/abc123xyz">isso</a></p>"#;
        assert_eq!(
            extract_first_url(html),
            Some("https://youtu.be/abc123xyz".to_owned())
        );
    }

    #[test]
    fn reads_meta_tags_in_either_attribute_order() {
        let a = r#"<meta property="og:title" content="Primeiro">"#;
        let b = r#"<meta content="Segundo" property="og:title">"#;
        let c = r#"<meta property='og:title' content='Terceiro'>"#;
        assert_eq!(meta_content(a, "og:title").as_deref(), Some("Primeiro"));
        assert_eq!(meta_content(b, "og:title").as_deref(), Some("Segundo"));
        assert_eq!(meta_content(c, "og:title").as_deref(), Some("Terceiro"));
    }

    #[test]
    fn builds_a_youtube_card_from_the_real_tag_shape() {
        // Copied from what youtube.com/watch actually served on 2026-08-06.
        let html = r#"
            <meta property="og:site_name" content="YouTube">
            <meta property="og:url" content="https://www.youtube.com/watch?v=dQw4w9WgXcQ">
            <meta property="og:title" content="Rick Astley - Never Gonna Give You Up">
            <meta property="og:image" content="https://i.ytimg.com/vi/dQw4w9WgXcQ/maxresdefault.jpg">
            <meta property="og:type" content="video.other">
        "#;
        let card = card_from_html(html, "https://youtu.be/dQw4w9WgXcQ");
        assert_eq!(card.site_name.as_deref(), Some("YouTube"));
        assert_eq!(
            card.image_url.as_deref(),
            Some("https://i.ytimg.com/vi/dQw4w9WgXcQ/maxresdefault.jpg"),
            "the thumbnail is the whole point of the feature"
        );
        assert_eq!(card.kind, "video");
        assert!(is_useful(&card));
    }

    #[test]
    fn falls_back_to_the_title_tag_when_there_is_no_open_graph() {
        let html = "<html><head><title>Uma página simples</title></head><body>x</body></html>";
        let card = card_from_html(html, "https://example.com/x");
        assert_eq!(card.title.as_deref(), Some("Uma página simples"));
        assert!(is_useful(&card), "a title alone is still worth a card");
    }

    #[test]
    fn a_page_with_nothing_useful_yields_no_card() {
        let card = card_from_html("<html><body>só texto</body></html>", "https://x.example/");
        assert!(!is_useful(&card), "an empty card must not be rendered");
    }

    #[test]
    fn decodes_the_entities_that_appear_in_titles() {
        let html =
            r#"<meta property="og:title" content="Lula &amp; Bolsonaro: o &quot;debate&quot;">"#;
        assert_eq!(
            meta_content(html, "og:title").as_deref(),
            Some(r#"Lula & Bolsonaro: o "debate""#)
        );
    }

    #[test]
    fn malformed_html_does_not_panic() {
        for junk in [
            "<meta property=\"og:title\"",
            "<meta property=og:title content=x>",
            "<<<>>>",
            "",
            "<title>",
        ] {
            let _ = card_from_html(junk, "https://x.example/");
            let _ = meta_content(junk, "og:title");
        }
    }
}

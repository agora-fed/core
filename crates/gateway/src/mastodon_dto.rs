//! # Mastodon Client API DTO shapes + converters (0.19.0).
//!
//! Third-party Mastodon clients (Ivory, Elk, Ice Cubes, Tusky) speak a very
//! specific JSON envelope. Every field name, order, and null-vs-absent
//! choice matters — an unknown key is fine but a missing REQUIRED one
//! breaks the client. This module encodes those shapes and provides
//! `from_*` converters from our internal DTOs.
//!
//! References: docs.joinmastodon.org/entities/{Status,Account,MediaAttachment,
//! Poll,Notification,Instance}. All `id` fields are STRINGS on the wire
//! (Mastodon casts ints to strings for JS bigint safety) — we serialize
//! UUIDs the same way.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::federation_feed::FeedItemDto;
use crate::note_media::MediaDto;
use crate::notifications::NotificationDto;
use crate::polls::PollDto;

// ---------------------------------------------------------------------------
// Account
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Account {
    pub id: String,
    pub username: String,
    /// For local: `username`. For remote: `username@host`.
    pub acct: String,
    pub display_name: String,
    pub locked: bool,
    pub bot: bool,
    pub group: bool,
    pub discoverable: Option<bool>,
    pub created_at: String,
    pub note: String,
    pub url: String,
    pub uri: String,
    pub avatar: String,
    pub avatar_static: String,
    pub header: String,
    pub header_static: String,
    pub followers_count: i64,
    pub following_count: i64,
    pub statuses_count: i64,
    pub last_status_at: Option<String>,
    pub emojis: Vec<serde_json::Value>,
    pub fields: Vec<serde_json::Value>,
}

/// The set of fields Mastodon expects from a citizen's profile. We keep our
/// own `ProfileDto` and derive `Account` per request; this struct captures
/// the minimum we need to look up.
#[derive(Debug, Clone)]
pub struct AccountBuild<'a> {
    pub citizen_id_str: String,
    pub handle: &'a str,
    pub display_name: Option<&'a str>,
    pub bio_html: Option<String>,
    pub avatar_url: Option<&'a str>,
    pub cover_url: Option<&'a str>,
    pub created_at: DateTime<Utc>,
    pub host: &'a str,
    pub followers_count: i64,
    pub following_count: i64,
    pub statuses_count: i64,
    pub last_status_at: Option<DateTime<Utc>>,
}

impl Account {
    #[must_use]
    pub fn from_local(b: AccountBuild<'_>) -> Self {
        let default_avatar = format!(
            "https://{}/media/avatars/default.png",
            b.host.trim_end_matches('/')
        );
        let default_header = format!(
            "https://{}/media/covers/default.png",
            b.host.trim_end_matches('/')
        );
        let avatar = b
            .avatar_url
            .map(str::to_owned)
            .unwrap_or_else(|| default_avatar.clone());
        let header = b
            .cover_url
            .map(str::to_owned)
            .unwrap_or_else(|| default_header.clone());
        let profile_url = format!(
            "https://{}/perfil/?u={}",
            b.host.trim_end_matches('/'),
            b.handle
        );
        let actor_url = format!("https://{}/actors/{}", b.host.trim_end_matches('/'), b.handle);
        Self {
            id: b.citizen_id_str,
            username: b.handle.to_owned(),
            acct: b.handle.to_owned(),
            display_name: b.display_name.unwrap_or("").to_owned(),
            locked: false,
            bot: false,
            group: false,
            discoverable: Some(true),
            created_at: b.created_at.to_rfc3339(),
            note: b.bio_html.unwrap_or_default(),
            url: profile_url,
            uri: actor_url,
            avatar,
            avatar_static: default_avatar,
            header,
            header_static: default_header,
            followers_count: b.followers_count,
            following_count: b.following_count,
            statuses_count: b.statuses_count,
            last_status_at: b.last_status_at.map(|d| d.format("%Y-%m-%d").to_string()),
            emojis: Vec::new(),
            fields: Vec::new(),
        }
    }

    /// Sparse Account built from what we know about a remote author on a
    /// feed row (the timeline_entry doesn't store much). Missing counts fall
    /// back to zero — Mastodon still renders the card.
    #[must_use]
    pub fn from_remote_stub(
        handle: &str,
        display_name: Option<&str>,
        avatar_url: Option<&str>,
        actor_url: &str,
    ) -> Self {
        let avatar = avatar_url
            .map(str::to_owned)
            .unwrap_or_else(|| "".to_owned());
        Self {
            id: format!("r-{}", short_hash(actor_url)),
            username: handle.split('@').next().unwrap_or(handle).to_owned(),
            acct: handle.to_owned(),
            display_name: display_name.unwrap_or("").to_owned(),
            locked: false,
            bot: false,
            group: false,
            discoverable: Some(true),
            created_at: chrono::Utc::now().to_rfc3339(),
            note: String::new(),
            url: actor_url.to_owned(),
            uri: actor_url.to_owned(),
            avatar: avatar.clone(),
            avatar_static: avatar,
            header: String::new(),
            header_static: String::new(),
            followers_count: 0,
            following_count: 0,
            statuses_count: 0,
            last_status_at: None,
            emojis: Vec::new(),
            fields: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// MediaAttachment
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct MastodonMedia {
    pub id: String,
    /// image | video | audio | gifv | unknown
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    pub preview_url: String,
    pub remote_url: Option<String>,
    pub description: Option<String>,
    pub blurhash: Option<String>,
    pub meta: serde_json::Value,
}

impl From<&MediaDto> for MastodonMedia {
    fn from(m: &MediaDto) -> Self {
        let meta = if let (Some(w), Some(h)) = (m.width, m.height) {
            serde_json::json!({
                "original": {
                    "width": w,
                    "height": h,
                    "size": format!("{w}x{h}"),
                    "aspect": if h > 0 { w as f64 / h as f64 } else { 1.0 },
                }
            })
        } else {
            serde_json::json!({})
        };
        Self {
            id: m.id.to_string(),
            kind: m.kind.clone(),
            url: m.url.clone(),
            preview_url: m.url.clone(),
            remote_url: None,
            description: m.alt_text.clone(),
            blurhash: None,
            meta,
        }
    }
}

// ---------------------------------------------------------------------------
// Poll
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct MastodonPoll {
    pub id: String,
    pub expires_at: Option<String>,
    pub expired: bool,
    pub multiple: bool,
    pub votes_count: i32,
    pub voters_count: Option<i32>,
    pub voted: bool,
    pub own_votes: Option<Vec<i32>>,
    pub options: Vec<serde_json::Value>,
    pub emojis: Vec<serde_json::Value>,
}

impl From<&PollDto> for MastodonPoll {
    fn from(p: &PollDto) -> Self {
        // Mastodon own_votes is the array of chosen option INDEXES (not ids).
        let own_votes: Vec<i32> = p
            .voted_option_ids
            .iter()
            .filter_map(|voted_id| {
                p.options
                    .iter()
                    .position(|o| o.id == *voted_id)
                    .map(|i| i as i32)
            })
            .collect();
        let voted = !own_votes.is_empty();
        let expired = p.closed_at.is_some() || p.expires_at < chrono::Utc::now();
        let options: Vec<serde_json::Value> = p
            .options
            .iter()
            .map(|o| {
                serde_json::json!({
                    "title": o.text,
                    "votes_count": o.vote_count,
                })
            })
            .collect();
        Self {
            id: p.id.to_string(),
            expires_at: Some(p.expires_at.to_rfc3339()),
            expired,
            multiple: p.multiple,
            votes_count: p.total_votes,
            voters_count: None,
            voted,
            own_votes: Some(own_votes),
            options,
            emojis: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub id: String,
    pub uri: String,
    pub url: Option<String>,
    pub account: Account,
    pub in_reply_to_id: Option<String>,
    pub in_reply_to_account_id: Option<String>,
    pub reblog: Option<Box<Status>>,
    pub content: String,
    pub created_at: String,
    pub edited_at: Option<String>,
    pub emojis: Vec<serde_json::Value>,
    pub replies_count: i64,
    pub reblogs_count: i64,
    pub favourites_count: i64,
    pub favourited: bool,
    pub reblogged: bool,
    pub muted: bool,
    pub bookmarked: bool,
    pub sensitive: bool,
    pub spoiler_text: String,
    pub visibility: String,
    pub media_attachments: Vec<MastodonMedia>,
    pub mentions: Vec<serde_json::Value>,
    pub tags: Vec<serde_json::Value>,
    pub card: Option<serde_json::Value>,
    pub poll: Option<MastodonPoll>,
    pub application: Option<serde_json::Value>,
    pub language: Option<String>,
    pub pinned: bool,
}

impl Status {
    /// Build a Status from a FeedItemDto. `id` and `in_reply_to_id` must be
    /// looked up (or created) via `mastodon_api::ensure_status_id` so the
    /// value round-trips future calls.
    #[must_use]
    pub fn from_feed_item(
        item: &FeedItemDto,
        id: String,
        in_reply_to_id: Option<String>,
        account: Account,
    ) -> Self {
        Self {
            id,
            uri: item.object_uri.clone(),
            url: Some(item.object_uri.clone()),
            account,
            in_reply_to_id,
            in_reply_to_account_id: None,
            reblog: None,
            content: item.content_html.clone(),
            created_at: item.published_at.to_rfc3339(),
            edited_at: item.edited_at.map(|d| d.to_rfc3339()),
            emojis: Vec::new(),
            replies_count: 0,
            reblogs_count: item.boost_count,
            favourites_count: item.like_count,
            favourited: item.liked_by_me,
            reblogged: item.boosted_by_me,
            muted: false,
            bookmarked: false,
            sensitive: item.sensitive,
            spoiler_text: item.spoiler_text.clone().unwrap_or_default(),
            visibility: "public".into(),
            media_attachments: item
                .attachments
                .iter()
                .map(MastodonMedia::from)
                .collect(),
            mentions: Vec::new(),
            tags: Vec::new(),
            card: None,
            poll: item.poll.as_ref().map(MastodonPoll::from),
            application: None,
            language: None,
            pinned: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Notification
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct MastodonNotification {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub created_at: String,
    pub account: Account,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
}

impl MastodonNotification {
    #[must_use]
    pub fn from_dto(
        n: &NotificationDto,
        account: Account,
        status: Option<Status>,
    ) -> Self {
        Self {
            id: n.id.to_string(),
            // Mastodon uses "favourite" and "reblog" — we already emit those
            // string values, so pass-through.
            kind: n.kind.clone(),
            created_at: n.created_at.to_rfc3339(),
            account,
            status,
        }
    }
}

// ---------------------------------------------------------------------------
// Instance
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Instance {
    pub uri: String,
    pub title: String,
    pub short_description: String,
    pub description: String,
    pub email: String,
    pub version: String,
    pub urls: serde_json::Value,
    pub stats: serde_json::Value,
    pub thumbnail: Option<String>,
    pub languages: Vec<String>,
    pub registrations: bool,
    pub approval_required: bool,
    pub invites_enabled: bool,
    pub configuration: serde_json::Value,
    pub contact_account: Option<Account>,
    pub rules: Vec<serde_json::Value>,
}

impl Instance {
    /// Build a static-ish description of the platform. Stats are supplied by
    /// the caller (short DB counts) so this can stay pure.
    #[must_use]
    pub fn build(
        host: &str,
        user_count: i64,
        status_count: i64,
        contact: Option<Account>,
    ) -> Self {
        let host = host.trim_end_matches('/');
        Self {
            uri: host.to_owned(),
            title: "DemocraciaBR".into(),
            short_description: "Rede social federada para participação política brasileira.".into(),
            description:
                "DemocraciaBR une o fediverso (ActivityPub) à democracia participativa (Decidim-class): publique, siga, cobre mandatos, proponha, vote, deleguie."
                    .into(),
            email: "sistema@democracia.social.br".into(),
            version: format!("4.2.0-compatible; DemocraciaBR/{}", env!("CARGO_PKG_VERSION")),
            urls: serde_json::json!({
                "streaming_api": format!("wss://{host}"),
            }),
            stats: serde_json::json!({
                "user_count": user_count,
                "status_count": status_count,
                "domain_count": 0,
            }),
            thumbnail: Some(format!("https://{host}/og-image.png")),
            languages: vec!["pt".into(), "en".into()],
            registrations: true,
            approval_required: false,
            invites_enabled: false,
            configuration: serde_json::json!({
                "statuses": { "max_characters": 5000 },
                "media_attachments": {
                    "supported_mime_types": ["image/png", "image/jpeg", "image/webp", "image/gif"],
                    "image_size_limit": 8 * 1024 * 1024,
                    "image_matrix_limit": 3_000_000,
                },
                "polls": {
                    "max_options": 8,
                    "max_characters_per_option": 200,
                    "min_expiration": 300,
                    "max_expiration": 7 * 24 * 60 * 60,
                    "allow_multiple": true,
                },
            }),
            contact_account: contact,
            rules: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Short deterministic id derived from a URI. Used when Mastodon expects a
/// STRING id for something we identify by URL (e.g. a Note without a numeric
/// pk). SHA-256 truncated to 22 chars of base64-url — collision-safe enough
/// for the visible ids third-party clients rehydrate.
#[must_use]
pub fn short_hash(input: &str) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    let out = h.finalize();
    URL_SAFE_NO_PAD.encode(&out[..16])
}

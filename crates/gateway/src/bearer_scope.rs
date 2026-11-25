//! OAuth scope check for bearer-authenticated requests (Mastodon Client API).
//!
//! `inject_identity` accepts an OAuth bearer as an alternate credential. A token carries the
//! scopes the user granted (`read`, `write`, `follow`, `push` — see
//! `mastodon_oauth::normalize_scopes_str`). This module decides if a request is inside them:
//!
//! * safe methods (GET, HEAD, OPTIONS) need `read`;
//! * mutating methods need `write`;
//! * `follow` also allows follow/block/mute changes (Mastodon's legacy `follow` scope);
//! * `push` also allows the Web Push subscription endpoints, both read and write.
//!
//! Cookie sessions are the site's own login and are NOT checked here.

use axum::http::Method;

/// Mastodon's error text for a token used outside its scopes (HTTP 403).
pub(crate) const OUT_OF_SCOPE: &str = "This action is outside the authorized scopes";

/// True when a token with `scopes` (space-separated) may make this request.
/// `path` may include the `/api/v1` prefix or not.
#[must_use]
pub(crate) fn permits(method: &Method, path: &str, scopes: &str) -> bool {
    let has = |s: &str| scopes.split_whitespace().any(|g| g == s);
    let path = path.strip_prefix("/api/v1").unwrap_or(path);
    if has("push") && is_push_path(path) {
        return true;
    }
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return has("read");
    }
    has("write") || (has("follow") && is_follow_path(path))
}

fn is_push_path(path: &str) -> bool {
    path.starts_with("/push/subscription") || path.starts_with("/me/push-subscriptions")
}

/// Relationship changes covered by Mastodon's `follow` scope.
fn is_follow_path(path: &str) -> bool {
    const ACCOUNT_ACTIONS: [&str; 6] = ["follow", "unfollow", "block", "unblock", "mute", "unmute"];
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    match segments.as_slice() {
        ["accounts", _, action] => ACCOUNT_ACTIONS.contains(action),
        ["follow_requests", ..] | ["domain_blocks", ..] => true,
        ["me", "follow" | "bulk_follow", ..] => true,
        ["me", "mutes" | "blocks", ..] => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_need_read() {
        assert!(permits(&Method::GET, "/api/v1/timelines/home", "read"));
        assert!(permits(&Method::HEAD, "/statuses/1", "read write"));
        assert!(!permits(&Method::GET, "/timelines/home", "write"));
        assert!(!permits(&Method::GET, "/timelines/home", "push"));
    }

    #[test]
    fn writes_need_write() {
        for m in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
            assert!(!permits(&m, "/api/v1/statuses", "read"), "{m}");
            assert!(permits(&m, "/api/v1/statuses", "read write"), "{m}");
        }
    }

    #[test]
    fn scope_names_are_matched_whole() {
        // "readwrite" is not "write"; no substring match.
        assert!(!permits(&Method::POST, "/statuses", "readwrite"));
        assert!(!permits(&Method::GET, "/statuses", "unread"));
        assert!(!permits(&Method::GET, "/statuses", ""));
    }

    #[test]
    fn follow_scope_covers_relationships_only() {
        assert!(permits(
            &Method::POST,
            "/api/v1/accounts/abc/follow",
            "read follow"
        ));
        assert!(permits(&Method::POST, "/accounts/abc/unmute", "follow"));
        assert!(permits(&Method::POST, "/me/follow", "follow"));
        assert!(permits(&Method::DELETE, "/me/blocks/url", "follow"));
        assert!(!permits(&Method::POST, "/statuses", "read follow"));
        assert!(!permits(&Method::POST, "/accounts/abc/note", "follow"));
        assert!(!permits(
            &Method::POST,
            "/accounts/abc/statuses/x",
            "follow"
        ));
    }

    #[test]
    fn push_scope_covers_push_subscriptions_only() {
        assert!(permits(
            &Method::POST,
            "/api/v1/me/push-subscriptions",
            "push"
        ));
        assert!(permits(&Method::GET, "/push/subscription", "push"));
        assert!(permits(&Method::DELETE, "/push/subscription", "push"));
        assert!(!permits(&Method::POST, "/statuses", "push"));
    }
}

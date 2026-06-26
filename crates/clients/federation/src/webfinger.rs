//! WebFinger (RFC 7033) resource resolution: `acct:user@host` -> the actor self-link.
//!
//! Pure builders returning `serde_json::Value`; the HTTP layer (`crate::http`) only adapts these
//! into responses. A request for `acct:user@host` resolves to a JRD whose `self` link points at the
//! actor document, which is how the fediverse bootstraps actor discovery from a handle.

use serde_json::{json, Value};

use crate::actor::actor_id;

/// The JRD link relation and media type for an actor's self link.
const REL_SELF: &str = "self";
const ACTIVITY_JSON: &str = "application/activity+json";

/// Why a WebFinger `resource` could not be resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebFingerError {
    /// The `resource` was not an `acct:` URI.
    NotAcctScheme,
    /// The `acct:` URI was not of the form `user@host`.
    Malformed,
    /// The resource's host did not match the instance host serving the request.
    HostMismatch,
}

impl std::fmt::Display for WebFingerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAcctScheme => write!(f, "resource is not an acct: URI"),
            Self::Malformed => write!(f, "acct: resource is not of the form user@host"),
            Self::HostMismatch => write!(f, "resource host does not match this instance"),
        }
    }
}

impl std::error::Error for WebFingerError {}

/// Parse an `acct:user@host` resource into `(user, host)`.
///
/// IPv6 hosts must be bracketed (`acct:user@[2001:db8::1]`); the bracketed form is preserved.
///
/// # Errors
/// Returns [`WebFingerError`] if the scheme is not `acct:` or the body is not `user@host`.
pub fn parse_acct(resource: &str) -> Result<(&str, &str), WebFingerError> {
    let body = resource
        .strip_prefix("acct:")
        .ok_or(WebFingerError::NotAcctScheme)?;
    // Split on the LAST '@' so an IPv6-bracketed host with no '@' is unaffected and any future
    // user-part containing '@' would still bind the host to the trailing authority.
    let (user, host) = body.rsplit_once('@').ok_or(WebFingerError::Malformed)?;
    if user.is_empty() || host.is_empty() {
        return Err(WebFingerError::Malformed);
    }
    Ok((user, host))
}

/// Build the JRD document for `resource`, served by the instance at `instance_host`.
///
/// The resource host must equal `instance_host` — an instance only answers for its own accounts.
///
/// # Errors
/// Returns [`WebFingerError`] on a bad resource or a host that this instance does not serve.
pub fn webfinger_jrd(resource: &str, instance_host: &str) -> Result<Value, WebFingerError> {
    let (user, host) = parse_acct(resource)?;
    if !host.eq_ignore_ascii_case(instance_host) {
        return Err(WebFingerError::HostMismatch);
    }
    Ok(json!({
        "subject": resource,
        "links": [
            {
                "rel": REL_SELF,
                "type": ACTIVITY_JSON,
                "href": actor_id(instance_host, user),
            }
        ]
    }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    // IPv6-first: the canonical example instance host is a bracketed IPv6 literal.
    const HOST: &str = "[2001:db8::1]";

    #[test]
    fn parses_acct_user_and_host() {
        let (user, host) = parse_acct("acct:mandate-1@[2001:db8::1]").unwrap();
        assert_eq!(user, "mandate-1");
        assert_eq!(host, "[2001:db8::1]");
    }

    #[test]
    fn rejects_non_acct_scheme() {
        assert_eq!(
            parse_acct("https://example/x").unwrap_err(),
            WebFingerError::NotAcctScheme
        );
    }

    #[test]
    fn rejects_malformed_acct() {
        assert_eq!(
            parse_acct("acct:nohost").unwrap_err(),
            WebFingerError::Malformed
        );
        assert_eq!(
            parse_acct("acct:@host").unwrap_err(),
            WebFingerError::Malformed
        );
    }

    #[test]
    fn jrd_points_self_link_at_actor_document() {
        let jrd = webfinger_jrd("acct:mandate-1@[2001:db8::1]", HOST).unwrap();
        assert_eq!(jrd["subject"], "acct:mandate-1@[2001:db8::1]");
        let link = &jrd["links"][0];
        assert_eq!(link["rel"], "self");
        assert_eq!(link["type"], "application/activity+json");
        assert_eq!(link["href"], "https://[2001:db8::1]/actors/mandate-1");
    }

    #[test]
    fn refuses_a_host_it_does_not_serve() {
        let err = webfinger_jrd("acct:mandate-1@other.example", HOST).unwrap_err();
        assert_eq!(err, WebFingerError::HostMismatch);
    }

    #[test]
    fn host_match_is_case_insensitive() {
        assert!(webfinger_jrd("acct:user@EXAMPLE.test", "example.test").is_ok());
    }
}

//! # Identity-provider client for petition signers (Zitadel v2 user API).
//!
//! The gateway uses this only when the operator turns provisioning on
//! (`PETITIONS_SSO_PROVISIONING=zitadel`). Every call goes through the SSRF guard in
//! [`crate::outbound`], because the API URL is operator configuration.
//!
//! The service account needs user-management rights in ONE organization (in Zitadel,
//! the `ORG_USER_MANAGER` role). Do not give it instance-wide rights.
//!
//! Facts this client relies on (checked against Zitadel 4.x):
//! * Zitadel does NOT keep e-mail addresses unique. Always search before creating.
//! * A human user created without a password is active and can set one later through
//!   the password-reset link.
//! * A verification code works once. A second try fails as "invalid".
//!
//! To add another provider, add a client with the same operations and a new value of
//! `PETITIONS_SSO_PROVISIONING`.

use serde::Deserialize;
use serde_json::{json, Value};

use super::config::ProviderCfg;
use crate::outbound::{guarded_send, OutboundPolicy};

/// A user found at the provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderUser {
    pub id: String,
    pub email_verified: bool,
}

/// How the new user's e-mail is handled.
pub(crate) enum EmailMode<'a> {
    /// The provider sends a code; the link follows this template (`{{.UserID}}`, `{{.Code}}`).
    SendCode(&'a str),
    /// This server already confirmed the address.
    Verified,
}

/// Result of a create call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Created {
    User(String),
    UsernameTaken,
}

/// A failed provider call. Never shown to the visitor.
#[derive(Debug)]
pub(crate) enum ProviderError {
    Transport(String),
    Status(u16),
    BadResponse,
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(e) => write!(f, "transport: {e}"),
            Self::Status(s) => write!(f, "provider answered HTTP {s}"),
            Self::BadResponse => write!(f, "provider answer could not be read"),
        }
    }
}

type R<T> = Result<T, ProviderError>;

/// The Zitadel client.
pub(crate) struct Zitadel<'a> {
    cfg: &'a ProviderCfg,
}

impl<'a> Zitadel<'a> {
    pub(crate) fn new(cfg: &'a ProviderCfg) -> Self {
        Self { cfg }
    }

    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> R<(u16, Value)> {
        let headers = vec![
            (
                "Authorization".to_owned(),
                format!("Bearer {}", self.cfg.token),
            ),
            ("Content-Type".to_owned(), "application/json".to_owned()),
            ("Accept".to_owned(), "application/json".to_owned()),
        ];
        let bytes = body.map(|b| b.to_string().into_bytes()).unwrap_or_default();
        let url = format!("{}{path}", self.cfg.api);
        let (status, raw) = guarded_send(method, &url, &headers, bytes, &OutboundPolicy::default())
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;
        let value = serde_json::from_slice(&raw).unwrap_or(Value::Null);
        Ok((status, value))
    }

    async fn ok(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> R<Value> {
        let (status, value) = self.call(method, path, body).await?;
        if (200..300).contains(&status) {
            Ok(value)
        } else {
            Err(ProviderError::Status(status))
        }
    }

    /// Find users of the organization with this address. A verified one comes first.
    pub(crate) async fn find_by_email(&self, email: &str) -> R<Option<ProviderUser>> {
        let body = json!({
            "queries": [
                {"emailQuery": {"emailAddress": email}},
                {"organizationIdQuery": {"organizationId": self.cfg.org_id}}
            ]
        });
        let value = self
            .ok(reqwest::Method::POST, "/v2/users", Some(body))
            .await?;
        Ok(pick_user(&value))
    }

    /// Create a human user with no password.
    pub(crate) async fn create_user(
        &self,
        username: &str,
        name: &str,
        email: &str,
        mode: EmailMode<'_>,
    ) -> R<Created> {
        let email_part = match mode {
            EmailMode::SendCode(template) => {
                json!({"email": email, "sendCode": {"urlTemplate": template}})
            }
            EmailMode::Verified => json!({"email": email, "isVerified": true}),
        };
        let body = json!({
            "organization": {"orgId": self.cfg.org_id},
            "username": username,
            "profile": {"givenName": name, "familyName": name, "displayName": name},
            "email": email_part,
        });
        let (status, value) = self
            .call(reqwest::Method::POST, "/v2/users/human", Some(body))
            .await?;
        match status {
            200 | 201 => value
                .get("userId")
                .and_then(Value::as_str)
                .map(|id| Created::User(id.to_owned()))
                .ok_or(ProviderError::BadResponse),
            409 => Ok(Created::UsernameTaken),
            other => Err(ProviderError::Status(other)),
        }
    }

    /// Send a new code to a user whose address is not verified yet.
    pub(crate) async fn resend_code(&self, user_id: &str, template: &str) -> R<()> {
        let body = json!({"sendCode": {"urlTemplate": template}});
        self.ok(
            reqwest::Method::POST,
            &format!("/v2/users/{}/email/resend", seg(user_id)),
            Some(body),
        )
        .await
        .map(|_| ())
    }

    /// Check a code. `Ok(false)` means the provider refused the code.
    pub(crate) async fn verify_email(&self, user_id: &str, code: &str) -> R<bool> {
        let body = json!({"verificationCode": code});
        let (status, _) = self
            .call(
                reqwest::Method::POST,
                &format!("/v2/users/{}/email/verify", seg(user_id)),
                Some(body),
            )
            .await?;
        match status {
            200..=299 => Ok(true),
            400 | 404 | 412 => Ok(false),
            other => Err(ProviderError::Status(other)),
        }
    }

    /// Ask the provider to e-mail a "choose your password" link.
    pub(crate) async fn send_password_link(&self, user_id: &str) -> R<()> {
        let body = json!({"sendLink": {"notificationType": "NOTIFICATION_TYPE_Email"}});
        self.ok(
            reqwest::Method::POST,
            &format!("/v2/users/{}/password_reset", seg(user_id)),
            Some(body),
        )
        .await
        .map(|_| ())
    }

    /// Is this user's address verified? A missing user counts as not verified.
    pub(crate) async fn email_verified(&self, user_id: &str) -> R<bool> {
        let path = format!("/v2/users/{}", seg(user_id));
        let (status, value) = self.call(reqwest::Method::GET, &path, None).await?;
        match status {
            200..=299 => Ok(value
                .pointer("/user/human/email/isVerified")
                .and_then(Value::as_bool)
                .unwrap_or(false)),
            404 => Ok(false),
            other => Err(ProviderError::Status(other)),
        }
    }

    /// Remove a user this server created, but only while its address is still
    /// unverified. A user that is gone already counts as removed.
    pub(crate) async fn delete_unverified(&self, user_id: &str) -> R<()> {
        if self.email_verified(user_id).await? {
            return Ok(());
        }
        let path = format!("/v2/users/{}", seg(user_id));
        let (status, _) = self.call(reqwest::Method::DELETE, &path, None).await?;
        match status {
            200..=299 | 404 => Ok(()),
            other => Err(ProviderError::Status(other)),
        }
    }
}

/// Provider ids are digits; refuse anything else so a stored value cannot change the path.
fn seg(id: &str) -> &str {
    if !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        id
    } else {
        "invalid"
    }
}

#[derive(Deserialize)]
struct Found {
    #[serde(rename = "userId")]
    user_id: String,
    #[serde(default)]
    human: Option<Human>,
}
#[derive(Deserialize)]
struct Human {
    #[serde(default)]
    email: Option<Email>,
}
#[derive(Deserialize)]
struct Email {
    #[serde(default, rename = "isVerified")]
    is_verified: bool,
}

/// From a search answer, take a verified user if one exists, else the first one.
fn pick_user(value: &Value) -> Option<ProviderUser> {
    let list: Vec<Found> = value
        .get("result")
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    let users: Vec<ProviderUser> = list
        .into_iter()
        .map(|f| ProviderUser {
            email_verified: f.human.and_then(|h| h.email).is_some_and(|e| e.is_verified),
            id: f.user_id,
        })
        .collect();
    users
        .iter()
        .find(|u| u.email_verified)
        .or_else(|| users.first())
        .cloned()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn search_prefers_the_verified_user() {
        let v = json!({"result": [
            {"userId": "1", "human": {"email": {"email": "a@b.c"}}},
            {"userId": "2", "human": {"email": {"email": "a@b.c", "isVerified": true}}}
        ]});
        assert_eq!(
            pick_user(&v),
            Some(ProviderUser {
                id: "2".into(),
                email_verified: true
            })
        );
    }

    #[test]
    fn search_falls_back_to_the_first_and_handles_empty() {
        let v = json!({"result": [{"userId": "7", "human": {"email": {"email": "a@b.c"}}}]});
        assert_eq!(
            pick_user(&v),
            Some(ProviderUser {
                id: "7".into(),
                email_verified: false
            })
        );
        assert_eq!(pick_user(&json!({"details": {}})), None);
        assert_eq!(pick_user(&Value::Null), None);
    }

    #[test]
    fn path_segments_are_sanitized() {
        assert_eq!(seg("399436623550350607"), "399436623550350607");
        assert_eq!(seg("../admin"), "invalid");
        assert_eq!(seg(""), "invalid");
    }

    #[test]
    fn errors_render() {
        for e in [
            ProviderError::Transport("x".into()),
            ProviderError::Status(500),
            ProviderError::BadResponse,
        ] {
            assert!(!e.to_string().is_empty());
        }
    }
}

//! # Operator configuration of public petition signing.
//!
//! Everything here comes from the environment, so each AGORA instance chooses its own
//! setup. The full guide is `docs/petitions.md`. Summary:
//!
//! | Variable | Values | Default |
//! |---|---|---|
//! | `PETITIONS_EMAIL_VERIFICATION` | `local` / `sso` | `local` |
//! | `PETITIONS_SSO_PROVISIONING` | `none` / `zitadel` | `none` |
//! | `PETITIONS_SSO_API` | base URL of the provider's API | `SSO_ISSUER` |
//! | `PETITIONS_SSO_TOKEN` | provider service-account token | — |
//! | `PETITIONS_SSO_ORG_ID` | organization that owns new users | `SSO_ALLOWED_ORG_ID` |
//! | `PETITIONS_HANDLE_CHECK_URL` | URL prefix; 404 means the username is free | — |
//! | `PETITIONS_PENDING_TTL_HOURS` | hours a confirmation link stays valid | `72` |
//! | `PETITIONS_RESEND_COOLDOWN_SECS` | wait before the same address gets another e-mail | `300` |
//! | `PETITIONS_MAX_EMAILS_PER_HOUR` | instance-wide cap of confirmation e-mails | `200` |
//! | `ALTCHA_HMAC_KEY` | secret for the proof-of-work check | — (signing off) |
//!
//! * `local` — this server sends the confirmation e-mail through `SMTP_*`. After the
//!   confirmation, the account is created here. When provisioning is on, the server also
//!   creates the user at the identity provider, with the e-mail already verified, and asks
//!   the provider to send a "choose your password" link.
//! * `sso` — the identity provider creates the user and sends its own verification code.
//!   The link in that e-mail comes back to this server, which checks the code with the
//!   provider. Use this when the provider already has a working mail setup and this
//!   server has none. It needs `PETITIONS_SSO_PROVISIONING`.

/// Who sends the confirmation e-mail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verification {
    /// This server, through its SMTP settings.
    Local,
    /// The identity provider, through its own mail setup.
    Sso,
}

/// Connection to an identity provider's admin API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderCfg {
    /// API base, no trailing slash, for example `https://sso.example.org`.
    pub api: String,
    /// Service-account bearer token. Needs only user-management rights in `org_id`.
    pub token: String,
    /// The organization that owns the users this server creates.
    pub org_id: String,
}

/// The whole petition-signing configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PetitionCfg {
    pub verification: Verification,
    /// `Some` when this server creates accounts at the identity provider (Zitadel today).
    pub provider: Option<ProviderCfg>,
    /// Optional username availability check (see the module docs).
    pub handle_check_url: Option<String>,
    pub pending_ttl_hours: i64,
    /// Seconds before the same address may get another e-mail for the same petition.
    pub resend_cooldown_secs: i64,
    /// Confirmation e-mails per hour for the whole instance (protects the mail reputation
    /// and the identity provider from bulk sign-ups).
    pub max_emails_per_hour: i64,
    /// `None` turns anonymous signing off; signed-in members can still sign.
    pub altcha_key: Option<Vec<u8>>,
    /// Public origin used in links, for example `https://agora.example.org`.
    pub origin: String,
}

/// Why a configuration is refused. Logged once; public signing then stays off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CfgError {
    UnknownVerification(String),
    UnknownProvider(String),
    SsoNeedsProvider,
    ProviderIncomplete,
}

impl std::fmt::Display for CfgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownVerification(v) => {
                write!(f, "PETITIONS_EMAIL_VERIFICATION={v}: use local or sso")
            }
            Self::UnknownProvider(v) => {
                write!(f, "PETITIONS_SSO_PROVISIONING={v}: use none or zitadel")
            }
            Self::SsoNeedsProvider => write!(
                f,
                "PETITIONS_EMAIL_VERIFICATION=sso needs PETITIONS_SSO_PROVISIONING=zitadel"
            ),
            Self::ProviderIncomplete => write!(
                f,
                "provisioning needs PETITIONS_SSO_API (or SSO_ISSUER), PETITIONS_SSO_TOKEN \
                 and PETITIONS_SSO_ORG_ID (or SSO_ALLOWED_ORG_ID)"
            ),
        }
    }
}

const DEFAULT_TTL_HOURS: i64 = 72;
const MAX_TTL_HOURS: i64 = 24 * 14;
const DEFAULT_COOLDOWN_SECS: i64 = 300;
const DEFAULT_MAX_PER_HOUR: i64 = 200;

/// Build the configuration from a lookup function. `std::env::var` in production; a map
/// in tests, so tests never touch the process environment.
pub(crate) fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<PetitionCfg, CfgError> {
    let val = |k: &str| {
        get(k)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };

    let verification = match val("PETITIONS_EMAIL_VERIFICATION").as_deref() {
        None | Some("local") => Verification::Local,
        Some("sso") => Verification::Sso,
        Some(other) => return Err(CfgError::UnknownVerification(other.to_owned())),
    };
    let provider = match val("PETITIONS_SSO_PROVISIONING").as_deref() {
        None | Some("none") => None,
        Some("zitadel") => {
            let api = val("PETITIONS_SSO_API").or_else(|| val("SSO_ISSUER"));
            let token = val("PETITIONS_SSO_TOKEN");
            let org_id = val("PETITIONS_SSO_ORG_ID").or_else(|| val("SSO_ALLOWED_ORG_ID"));
            match (api, token, org_id) {
                (Some(api), Some(token), Some(org_id)) => Some(ProviderCfg {
                    api: api.trim_end_matches('/').to_owned(),
                    token,
                    org_id,
                }),
                _ => return Err(CfgError::ProviderIncomplete),
            }
        }
        Some(other) => return Err(CfgError::UnknownProvider(other.to_owned())),
    };
    if verification == Verification::Sso && provider.is_none() {
        return Err(CfgError::SsoNeedsProvider);
    }
    let pending_ttl_hours = val("PETITIONS_PENDING_TTL_HOURS")
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|h| (1..=MAX_TTL_HOURS).contains(h))
        .unwrap_or(DEFAULT_TTL_HOURS);
    let origin = val("PUBLIC_ORIGIN")
        .unwrap_or_else(|| "http://localhost:4321".to_owned())
        .trim_end_matches('/')
        .to_owned();
    Ok(PetitionCfg {
        verification,
        provider,
        handle_check_url: val("PETITIONS_HANDLE_CHECK_URL"),
        pending_ttl_hours,
        resend_cooldown_secs: bounded(
            &val,
            "PETITIONS_RESEND_COOLDOWN_SECS",
            0..=86_400,
            DEFAULT_COOLDOWN_SECS,
        ),
        max_emails_per_hour: bounded(
            &val,
            "PETITIONS_MAX_EMAILS_PER_HOUR",
            1..=100_000,
            DEFAULT_MAX_PER_HOUR,
        ),
        altcha_key: val("ALTCHA_HMAC_KEY").map(String::into_bytes),
        origin,
    })
}

/// A number from the environment inside `range`, else `default`.
fn bounded(
    val: &impl Fn(&str) -> Option<String>,
    key: &str,
    range: std::ops::RangeInclusive<i64>,
    default: i64,
) -> i64 {
    val(key)
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|n| range.contains(n))
        .unwrap_or(default)
}

/// Read the configuration from the process environment.
pub(crate) fn from_env() -> Result<PetitionCfg, CfgError> {
    from_lookup(|k| std::env::var(k).ok())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn cfg(pairs: &[(&str, &str)]) -> Result<PetitionCfg, CfgError> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        from_lookup(|k| map.get(k).cloned())
    }

    #[test]
    fn defaults_to_local_without_provider_and_without_altcha() {
        let c = cfg(&[]).unwrap();
        assert_eq!(c.verification, Verification::Local);
        assert!(c.provider.is_none());
        assert!(c.altcha_key.is_none());
        assert_eq!(c.pending_ttl_hours, DEFAULT_TTL_HOURS);
        assert_eq!(c.resend_cooldown_secs, DEFAULT_COOLDOWN_SECS);
        assert_eq!(c.max_emails_per_hour, DEFAULT_MAX_PER_HOUR);
    }

    #[test]
    fn limits_are_read_and_bounded() {
        let c = cfg(&[
            ("PETITIONS_RESEND_COOLDOWN_SECS", "60"),
            ("PETITIONS_MAX_EMAILS_PER_HOUR", "0"),
        ])
        .unwrap();
        assert_eq!(c.resend_cooldown_secs, 60);
        assert_eq!(
            c.max_emails_per_hour, DEFAULT_MAX_PER_HOUR,
            "0 is out of range"
        );
    }

    #[test]
    fn sso_mode_reuses_the_login_settings() {
        let c = cfg(&[
            ("PETITIONS_EMAIL_VERIFICATION", "sso"),
            ("PETITIONS_SSO_PROVISIONING", "zitadel"),
            ("SSO_ISSUER", "https://sso.example.org/"),
            ("SSO_ALLOWED_ORG_ID", "42"),
            ("PETITIONS_SSO_TOKEN", "t"),
            ("PUBLIC_ORIGIN", "https://agora.example.org/"),
        ])
        .unwrap();
        assert_eq!(c.verification, Verification::Sso);
        let p = c.provider.unwrap();
        assert_eq!(p.api, "https://sso.example.org");
        assert_eq!(p.org_id, "42");
        assert_eq!(c.origin, "https://agora.example.org");
    }

    #[test]
    fn explicit_provider_settings_win() {
        let c = cfg(&[
            ("PETITIONS_SSO_PROVISIONING", "zitadel"),
            ("SSO_ISSUER", "https://login.example.org"),
            ("PETITIONS_SSO_API", "https://api.example.org"),
            ("SSO_ALLOWED_ORG_ID", "1"),
            ("PETITIONS_SSO_ORG_ID", "2"),
            ("PETITIONS_SSO_TOKEN", "t"),
        ])
        .unwrap();
        let p = c.provider.unwrap();
        assert_eq!(
            (p.api.as_str(), p.org_id.as_str()),
            ("https://api.example.org", "2")
        );
    }

    #[test]
    fn refuses_bad_or_incomplete_settings() {
        assert_eq!(
            cfg(&[("PETITIONS_EMAIL_VERIFICATION", "carrier-pigeon")]),
            Err(CfgError::UnknownVerification("carrier-pigeon".to_owned()))
        );
        assert_eq!(
            cfg(&[("PETITIONS_SSO_PROVISIONING", "keycloak")]),
            Err(CfgError::UnknownProvider("keycloak".to_owned()))
        );
        assert_eq!(
            cfg(&[("PETITIONS_EMAIL_VERIFICATION", "sso")]),
            Err(CfgError::SsoNeedsProvider)
        );
        assert_eq!(
            cfg(&[("PETITIONS_SSO_PROVISIONING", "zitadel")]),
            Err(CfgError::ProviderIncomplete)
        );
        for e in [
            CfgError::UnknownVerification("x".into()),
            CfgError::UnknownProvider("x".into()),
            CfgError::SsoNeedsProvider,
            CfgError::ProviderIncomplete,
        ] {
            assert!(!e.to_string().is_empty());
        }
    }

    #[test]
    fn ttl_is_bounded() {
        assert_eq!(
            cfg(&[("PETITIONS_PENDING_TTL_HOURS", "0")])
                .unwrap()
                .pending_ttl_hours,
            72
        );
        assert_eq!(
            cfg(&[("PETITIONS_PENDING_TTL_HOURS", "9999")])
                .unwrap()
                .pending_ttl_hours,
            72
        );
        assert_eq!(
            cfg(&[("PETITIONS_PENDING_TTL_HOURS", "24")])
                .unwrap()
                .pending_ttl_hours,
            24
        );
    }
}

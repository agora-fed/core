//! Pure identity domain: OIDC token validation, verification-level mapping, and the
//! small value-logic the service and HTTP layers build on. No `sqlx`, no `axum` — every
//! function here is deterministic and unit-tested (TESTING.md: time is injected, never
//! read ambiently).

use std::fmt;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};

use dsoc_core::events::Event;
use dsoc_core::ids::CitizenId;
use dsoc_core::{Error, Result, VerificationLevel};

/// Default session lifetime when none is configured (seconds). One hour mirrors a typical
/// OIDC access-token window; the gateway may override via `AUTH_SESSION_TTL_SECS`.
pub const DEFAULT_SESSION_TTL_SECS: i64 = 3600;

/// The dependency name surfaced in [`Error::Dependency`] for identity failures (never sensitive).
pub const IDENTITY_DEPENDENCY: &str = "zitadel";

/// Canonical string form of a [`VerificationLevel`] as stored in PostgreSQL `text` columns
/// (matches the `CHECK` constraints in the baseline and the `0100_auth` migration).
#[must_use]
pub const fn level_as_str(level: VerificationLevel) -> &'static str {
    match level {
        VerificationLevel::Anonymous => "anonymous",
        VerificationLevel::Email => "email",
        VerificationLevel::Directory => "directory",
        VerificationLevel::Strong => "strong",
    }
}

/// Parse a [`VerificationLevel`] from its stored `text` form.
///
/// # Errors
/// [`Error::Validation`] if `raw` is not one of the four sanctioned levels — a corrupt row
/// must fail loudly rather than silently degrade trust.
pub fn level_from_str(raw: &str) -> Result<VerificationLevel> {
    match raw {
        "anonymous" => Ok(VerificationLevel::Anonymous),
        "email" => Ok(VerificationLevel::Email),
        "directory" => Ok(VerificationLevel::Directory),
        "strong" => Ok(VerificationLevel::Strong),
        other => Err(Error::Validation(format!(
            "unknown verification level: {other}"
        ))),
    }
}

/// Whether a move from `current` to `target` is a genuine *upgrade* (a strictly higher
/// assurance level). Downgrades and no-ops return `false` and must not emit an upgrade event.
#[must_use]
pub const fn is_upgrade(current: VerificationLevel, target: VerificationLevel) -> bool {
    (target as u8) > (current as u8)
}

/// Whether `auth` consumes this event. Per the crate contract it reacts only to
/// `mandates.official.invited` (an invited official is a directory-verifiable identity).
#[must_use]
pub const fn consumes(event: &Event) -> bool {
    matches!(event, Event::MandateOfficialInvited { .. })
}

/// Compute a session's expiry from the injected clock instant and a TTL in seconds.
#[must_use]
pub fn compute_expiry(issued_at: DateTime<Utc>, ttl_secs: i64) -> DateTime<Utc> {
    issued_at + Duration::seconds(ttl_secs)
}

/// Derive a stable, public ActivityPub-readiness handle for a citizen (ADR-0005). It is a
/// function of the immutable [`CitizenId`] only, so it never changes and never leaks the
/// OIDC subject. The local-part is lowercase hex; an instance domain is appended later by
/// the federation layer.
#[must_use]
pub fn public_handle(citizen: CitizenId) -> String {
    format!("u-{}", citizen.as_uuid().simple())
}

/// The subset of OIDC ID-token claims this platform relies on. Unknown claims are ignored
/// (no `deny_unknown_fields`) so we interoperate with any conformant issuer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcClaims {
    /// Subject — the sovereign, stable identity of the principal (Zitadel `sub`).
    pub sub: String,
    /// Issuer — must match the configured sovereign issuer.
    pub iss: String,
    /// Expiry (seconds since the Unix epoch). Validated against the injected clock.
    pub exp: i64,
    /// Issued-at (seconds since the Unix epoch), when present.
    #[serde(default)]
    pub iat: Option<i64>,
}

/// A token that passed signature, issuer, audience, and expiry validation.
#[derive(Debug, Clone)]
pub struct ValidatedToken {
    /// The sovereign subject identifier.
    pub subject: String,
    /// The validated issuer.
    pub issuer: String,
    /// Token expiry as a UTC instant.
    pub expires_at: DateTime<Utc>,
}

/// Source of RS256/asymmetric verification keys for a configurable issuer. Abstracting this
/// behind a trait lets production resolve keys from a Zitadel JWKS endpoint while tests use a
/// local key — so the suite never requires a live Zitadel (CRATE spec).
#[async_trait::async_trait]
pub trait KeySource: Send + Sync {
    /// Resolve the decoding key for the given key id (`kid`), if any.
    ///
    /// # Errors
    /// [`Error::Dependency`] when the key material cannot be obtained (e.g. JWKS fetch fails),
    /// or [`Error::Unauthorized`] when no key matches the presented `kid`.
    async fn decoding_key(&self, kid: Option<&str>) -> Result<DecodingKey>;

    /// The signing algorithm to accept (e.g. RS256).
    fn algorithm(&self) -> Algorithm;

    /// The exact issuer string a token's `iss` claim must match.
    fn issuer(&self) -> &str;

    /// The audience a token's `aud` claim must contain, if audience validation is enabled.
    fn expected_audience(&self) -> Option<&str>;
}

/// A [`KeySource`] backed by a single, statically-held RS256 public key. Used in tests and
/// as a sovereign-pinned-key deployment option (no JWKS round-trip).
pub struct StaticKeySource {
    key: DecodingKey,
    algorithm: Algorithm,
    issuer: String,
    audience: Option<String>,
}

impl StaticKeySource {
    /// Build a source from a PEM-encoded RSA public key (a sovereign-pinned-key deployment).
    ///
    /// # Errors
    /// [`Error::Validation`] if the PEM cannot be parsed.
    pub fn from_rsa_pem(
        pem: &[u8],
        issuer: impl Into<String>,
        audience: Option<String>,
    ) -> Result<Self> {
        let key = DecodingKey::from_rsa_pem(pem)
            .map_err(|e| Error::Validation(format!("invalid RSA public key PEM: {e}")))?;
        Ok(Self::from_key(key, issuer, audience))
    }

    /// Build a source from a DER-encoded (PKCS#1 `RSAPublicKey`) RSA public key.
    ///
    /// # Errors
    /// [`Error::Validation`] if the DER cannot be parsed.
    pub fn from_rsa_der(
        der: &[u8],
        issuer: impl Into<String>,
        audience: Option<String>,
    ) -> Result<Self> {
        let key = DecodingKey::from_rsa_der(der);
        Ok(Self::from_key(key, issuer, audience))
    }

    fn from_key(key: DecodingKey, issuer: impl Into<String>, audience: Option<String>) -> Self {
        Self {
            key,
            algorithm: Algorithm::RS256,
            issuer: issuer.into(),
            audience,
        }
    }
}

impl fmt::Debug for StaticKeySource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StaticKeySource")
            .field("algorithm", &self.algorithm)
            .field("issuer", &self.issuer)
            .field("audience", &self.audience)
            .finish_non_exhaustive()
    }
}

#[async_trait::async_trait]
impl KeySource for StaticKeySource {
    async fn decoding_key(&self, _kid: Option<&str>) -> Result<DecodingKey> {
        Ok(self.key.clone())
    }

    fn algorithm(&self) -> Algorithm {
        self.algorithm
    }

    fn issuer(&self) -> &str {
        &self.issuer
    }

    fn expected_audience(&self) -> Option<&str> {
        self.audience.as_deref()
    }
}

/// Validates OIDC JWTs against a [`KeySource`]. Expiry is checked against an *injected*
/// instant rather than the ambient wall clock, so SLA-style determinism holds in tests
/// (TESTING.md). Signature, issuer, and audience are enforced by `jsonwebtoken`.
#[derive(Clone)]
pub struct TokenValidator {
    keys: Arc<dyn KeySource>,
}

impl fmt::Debug for TokenValidator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenValidator")
            .field("issuer", &self.keys.issuer())
            .finish_non_exhaustive()
    }
}

impl TokenValidator {
    /// Wrap a key source.
    #[must_use]
    pub fn new(keys: Arc<dyn KeySource>) -> Self {
        Self { keys }
    }

    /// Validate a bearer token at instant `now`.
    ///
    /// # Errors
    /// - [`Error::Unauthorized`] for a malformed, mis-signed, wrong-issuer, wrong-audience,
    ///   or expired token (the public surface never distinguishes *why*, to avoid an oracle).
    /// - [`Error::Dependency`] if the key source itself fails (logged, not surfaced raw).
    pub async fn validate(&self, token: &str, now: DateTime<Utc>) -> Result<ValidatedToken> {
        let header = decode_header(token).map_err(|_| Error::Unauthorized)?;
        let key = self.keys.decoding_key(header.kid.as_deref()).await?;

        let mut validation = Validation::new(self.keys.algorithm());
        validation.set_issuer(&[self.keys.issuer()]);
        match self.keys.expected_audience() {
            Some(aud) => validation.set_audience(&[aud]),
            None => validation.validate_aud = false,
        }
        // Expiry is enforced explicitly below against the injected clock for determinism.
        validation.validate_exp = false;

        let data =
            decode::<OidcClaims>(token, &key, &validation).map_err(|_| Error::Unauthorized)?;
        let claims = data.claims;

        if now.timestamp() >= claims.exp {
            return Err(Error::Unauthorized);
        }

        let expires_at =
            DateTime::<Utc>::from_timestamp(claims.exp, 0).ok_or(Error::Unauthorized)?;
        Ok(ValidatedToken {
            subject: claims.sub,
            issuer: claims.iss,
            expires_at,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};

    // Test-only RS256 keypair, stored as binary DER (PKCS#1) — NOT a secret and NOT PEM-armored,
    // generated offline solely to sign/verify fixtures in this suite, never used in production.
    const TEST_PRIV_DER: &[u8] = include_bytes!("../tests/data/test_priv.der");
    const TEST_PUB_DER: &[u8] = include_bytes!("../tests/data/test_pub.der");
    const OTHER_PRIV_DER: &[u8] = include_bytes!("../tests/data/other_priv.der");

    const ISSUER: &str = "https://id.democracia.social.example";

    fn fixed(ts: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(ts)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[derive(Serialize)]
    struct TestClaims {
        sub: String,
        iss: String,
        exp: i64,
        iat: i64,
    }

    fn sign(priv_der: &[u8], claims: &TestClaims) -> String {
        let key = EncodingKey::from_rsa_der(priv_der);
        encode(&Header::new(Algorithm::RS256), claims, &key).unwrap()
    }

    fn validator(audience: Option<String>) -> TokenValidator {
        let src = StaticKeySource::from_rsa_der(TEST_PUB_DER, ISSUER, audience).unwrap();
        TokenValidator::new(Arc::new(src))
    }

    #[test]
    fn level_string_roundtrips_for_every_variant() {
        for lvl in [
            VerificationLevel::Anonymous,
            VerificationLevel::Email,
            VerificationLevel::Directory,
            VerificationLevel::Strong,
        ] {
            assert_eq!(level_from_str(level_as_str(lvl)).unwrap(), lvl);
        }
    }

    #[test]
    fn level_from_str_rejects_unknown() {
        let err = level_from_str("president").unwrap_err();
        assert_eq!(err.code(), "invalid_input");
    }

    #[test]
    fn is_upgrade_is_strict_and_ordered() {
        assert!(is_upgrade(
            VerificationLevel::Anonymous,
            VerificationLevel::Email
        ));
        assert!(is_upgrade(
            VerificationLevel::Email,
            VerificationLevel::Strong
        ));
        // No-op and downgrade are not upgrades.
        assert!(!is_upgrade(
            VerificationLevel::Directory,
            VerificationLevel::Directory
        ));
        assert!(!is_upgrade(
            VerificationLevel::Strong,
            VerificationLevel::Email
        ));
    }

    #[test]
    fn consumes_only_official_invited() {
        use dsoc_core::ids::{MandateId, ProposalId};
        assert!(consumes(&Event::MandateOfficialInvited {
            mandate: MandateId::new()
        }));
        assert!(!consumes(&Event::ModerationFlagged {
            proposal: ProposalId::new()
        }));
    }

    #[test]
    fn expiry_is_issued_plus_ttl() {
        let now = fixed("2026-06-25T12:00:00Z");
        let exp = compute_expiry(now, 3600);
        assert_eq!(exp, fixed("2026-06-25T13:00:00Z"));
    }

    #[test]
    fn public_handle_is_stable_and_prefixed() {
        let c = CitizenId::new();
        let h1 = public_handle(c);
        let h2 = public_handle(c);
        assert_eq!(h1, h2);
        assert!(h1.starts_with("u-"));
        // Compact hex local-part: exactly the single prefix hyphen, no uuid dashes.
        assert_eq!(h1.matches('-').count(), 1);
    }

    #[tokio::test]
    async fn valid_token_yields_subject() {
        let now = fixed("2026-06-25T12:00:00Z");
        let token = sign(
            TEST_PRIV_DER,
            &TestClaims {
                sub: "ze-do-caixao".into(),
                iss: ISSUER.into(),
                exp: now.timestamp() + 3600,
                iat: now.timestamp(),
            },
        );
        let vt = validator(None).validate(&token, now).await.unwrap();
        assert_eq!(vt.subject, "ze-do-caixao");
        assert_eq!(vt.issuer, ISSUER);
    }

    #[tokio::test]
    async fn expired_token_is_unauthorized() {
        let now = fixed("2026-06-25T12:00:00Z");
        let token = sign(
            TEST_PRIV_DER,
            &TestClaims {
                sub: "expired".into(),
                iss: ISSUER.into(),
                exp: now.timestamp() - 1,
                iat: now.timestamp() - 3600,
            },
        );
        let err = validator(None).validate(&token, now).await.unwrap_err();
        assert!(matches!(err, Error::Unauthorized));
    }

    #[tokio::test]
    async fn wrong_signature_is_unauthorized() {
        let now = fixed("2026-06-25T12:00:00Z");
        // Signed with an unrelated key; the static source only trusts TEST_PUB_DER.
        let token = sign(
            OTHER_PRIV_DER,
            &TestClaims {
                sub: "impostor".into(),
                iss: ISSUER.into(),
                exp: now.timestamp() + 3600,
                iat: now.timestamp(),
            },
        );
        let err = validator(None).validate(&token, now).await.unwrap_err();
        assert!(matches!(err, Error::Unauthorized));
    }

    #[tokio::test]
    async fn wrong_issuer_is_unauthorized() {
        let now = fixed("2026-06-25T12:00:00Z");
        let token = sign(
            TEST_PRIV_DER,
            &TestClaims {
                sub: "valid-key".into(),
                iss: "https://evil.example".into(),
                exp: now.timestamp() + 3600,
                iat: now.timestamp(),
            },
        );
        let err = validator(None).validate(&token, now).await.unwrap_err();
        assert!(matches!(err, Error::Unauthorized));
    }

    #[tokio::test]
    async fn malformed_token_is_unauthorized() {
        let now = fixed("2026-06-25T12:00:00Z");
        let err = validator(None)
            .validate("not.a.jwt", now)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Unauthorized));
    }

    #[tokio::test]
    async fn audience_mismatch_is_unauthorized() {
        #[derive(Serialize)]
        struct AudClaims {
            sub: String,
            iss: String,
            exp: i64,
            aud: String,
        }

        let now = fixed("2026-06-25T12:00:00Z");
        // Token is addressed to a different audience than the validator accepts -> rejected.
        let key = EncodingKey::from_rsa_der(TEST_PRIV_DER);
        let token = encode(
            &Header::new(Algorithm::RS256),
            &AudClaims {
                sub: "wrong-aud".into(),
                iss: ISSUER.into(),
                exp: now.timestamp() + 3600,
                aud: "some-other-app".into(),
            },
            &key,
        )
        .unwrap();
        let err = validator(Some("democracia-mobile".into()))
            .validate(&token, now)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Unauthorized));
    }
}

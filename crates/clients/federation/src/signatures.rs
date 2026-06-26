//! HTTP Signatures (draft-cavage-http-signatures) types and the pure signing-string builder.
//!
//! This is a **foundation**: it models the actor `publicKey`, parses the `Signature` header, and
//! builds the canonical signing string deterministically. It deliberately does **not** perform any
//! cryptographic verification or live network delivery — that lands in Phase 3. Verification is
//! expressed as a [`SignatureVerifier`] trait so a crypto backend can be injected later without
//! reshaping the call sites.

use serde::{Deserialize, Serialize};

/// An ActivityPub actor public key (the `publicKey` object, defined by the security `@context`).
///
/// The PEM is a public key only — this crate never holds or emits private key material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicKey {
    /// Key id, conventionally `{actor-id}#main-key`; also the `keyId` in the `Signature` header.
    pub id: String,
    /// The actor that owns this key (its `id`).
    pub owner: String,
    /// PEM-encoded SPKI public key.
    #[serde(rename = "publicKeyPem")]
    pub public_key_pem: String,
}

impl PublicKey {
    /// Build the conventional `{actor_id}#main-key` key for `actor_id`.
    #[must_use]
    pub fn main_key(actor_id: &str, public_key_pem: impl Into<String>) -> Self {
        Self {
            id: format!("{actor_id}#main-key"),
            owner: actor_id.to_owned(),
            public_key_pem: public_key_pem.into(),
        }
    }
}

/// Signature algorithm advertised in the `Signature` header's `algorithm` parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SignatureAlgorithm {
    /// RSASSA-PKCS1-v1_5 with SHA-256 — the fediverse default.
    RsaSha256,
    /// Ed25519 — increasingly used by newer implementations.
    Ed25519,
}

impl SignatureAlgorithm {
    /// The wire token used in the `algorithm="..."` parameter.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RsaSha256 => "rsa-sha256",
            Self::Ed25519 => "ed25519",
        }
    }
}

/// A parsed `Signature` request header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureHeader {
    /// `keyId` — which key signed the request (resolves to a [`PublicKey`]).
    pub key_id: String,
    /// `algorithm` token, if present.
    pub algorithm: Option<String>,
    /// Ordered, space-separated list of (lowercased) header names that were signed.
    pub headers: Vec<String>,
    /// Base64 signature bytes (still opaque — verification is out of scope here).
    pub signature: String,
}

/// Error parsing a `Signature` header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureParseError {
    /// A required parameter (`keyId` or `signature`) was missing.
    MissingParameter(&'static str),
    /// The header was syntactically malformed.
    Malformed,
}

impl std::fmt::Display for SignatureParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingParameter(name) => write!(f, "missing signature parameter: {name}"),
            Self::Malformed => write!(f, "malformed Signature header"),
        }
    }
}

impl std::error::Error for SignatureParseError {}

impl SignatureHeader {
    /// Parse a `Signature` header value of the form
    /// `keyId="...",algorithm="rsa-sha256",headers="(request-target) host date",signature="..."`.
    ///
    /// Defaults `headers` to `["date"]` when absent, per the specification.
    ///
    /// # Errors
    /// Returns [`SignatureParseError`] when `keyId`/`signature` are missing or the header is malformed.
    pub fn parse(value: &str) -> Result<Self, SignatureParseError> {
        let mut key_id = None;
        let mut algorithm = None;
        let mut headers = None;
        let mut signature = None;

        for part in value.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let (name, raw) = part.split_once('=').ok_or(SignatureParseError::Malformed)?;
            let val = raw.trim().trim_matches('"');
            match name.trim() {
                "keyId" => key_id = Some(val.to_owned()),
                "algorithm" => algorithm = Some(val.to_owned()),
                "headers" => {
                    headers = Some(
                        val.split_whitespace()
                            .map(|h| h.to_ascii_lowercase())
                            .collect::<Vec<_>>(),
                    );
                }
                "signature" => signature = Some(val.to_owned()),
                _ => {}
            }
        }

        Ok(Self {
            key_id: key_id.ok_or(SignatureParseError::MissingParameter("keyId"))?,
            algorithm,
            headers: headers.unwrap_or_else(|| vec!["date".to_owned()]),
            signature: signature.ok_or(SignatureParseError::MissingParameter("signature"))?,
        })
    }
}

/// The special pseudo-header naming the request line in the signing string.
pub const REQUEST_TARGET: &str = "(request-target)";

/// Error building a signing string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SigningError {
    /// A header named in `covered` was not present in the supplied header set.
    MissingHeader(String),
}

impl std::fmt::Display for SigningError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingHeader(name) => write!(f, "covered header not present: {name}"),
        }
    }
}

impl std::error::Error for SigningError {}

/// Build the canonical signing string for the given covered headers, in order.
///
/// Each line is `name: value`, lowercased name; the pseudo-header `(request-target)` expands to
/// `{method-lowercase} {path}`. Lines are joined with `\n` and there is no trailing newline — this
/// is the exact byte sequence a signer hashes, so it must be deterministic.
///
/// `headers` is the request's header set as `(name, value)` pairs; lookup is case-insensitive.
///
/// # Errors
/// Returns [`SigningError::MissingHeader`] if a name in `covered` is absent from `headers`.
pub fn build_signing_string(
    method: &str,
    target: &str,
    headers: &[(String, String)],
    covered: &[&str],
) -> Result<String, SigningError> {
    let mut lines = Vec::with_capacity(covered.len());
    for name in covered {
        let lname = name.to_ascii_lowercase();
        if lname == REQUEST_TARGET {
            lines.push(format!(
                "{REQUEST_TARGET}: {} {target}",
                method.to_ascii_lowercase()
            ));
            continue;
        }
        let value = headers
            .iter()
            .find(|(h, _)| h.eq_ignore_ascii_case(&lname))
            .map(|(_, v)| v.as_str())
            .ok_or_else(|| SigningError::MissingHeader(lname.clone()))?;
        lines.push(format!("{lname}: {value}"));
    }
    Ok(lines.join("\n"))
}

/// Error returned by a signature verifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    /// The signature did not match the signing string for the given key.
    Mismatch,
    /// The verifier could not process the request (e.g. unsupported algorithm).
    Unsupported,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mismatch => write!(f, "signature does not verify against the key"),
            Self::Unsupported => write!(f, "unsupported signature algorithm or key"),
        }
    }
}

impl std::error::Error for VerifyError {}

/// Verification interface. A Phase-3 crypto backend implements this; the rest of the federation
/// surface depends only on the trait, so it can be unit-tested with a stub and swapped without
/// reshaping callers. No implementation here performs live network or cryptographic work.
pub trait SignatureVerifier {
    /// Verify that `signature_b64` is a valid signature of `signing_string` under `key`.
    ///
    /// # Errors
    /// Returns [`VerifyError`] when the signature does not verify or cannot be processed.
    fn verify(
        &self,
        signing_string: &str,
        signature_b64: &str,
        key: &PublicKey,
    ) -> Result<(), VerifyError>;
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn headers() -> Vec<(String, String)> {
        // IPv6-first: the example host is a bracketed IPv6 literal (PLAN.md DO-NOT IPv4 default).
        vec![
            ("Host".to_owned(), "[2001:db8::1]".to_owned()),
            (
                "Date".to_owned(),
                "Tue, 25 Jun 2026 12:00:00 GMT".to_owned(),
            ),
        ]
    }

    #[test]
    fn signing_string_is_canonical_and_ordered() {
        let s = build_signing_string(
            "GET",
            "/.well-known/webfinger",
            &headers(),
            &["(request-target)", "host", "date"],
        )
        .unwrap();
        assert_eq!(
            s,
            "(request-target): get /.well-known/webfinger\n\
             host: [2001:db8::1]\n\
             date: Tue, 25 Jun 2026 12:00:00 GMT"
        );
    }

    #[test]
    fn signing_string_lowercases_header_names_case_insensitively() {
        let s = build_signing_string("POST", "/inbox", &headers(), &["HOST"]).unwrap();
        assert_eq!(s, "host: [2001:db8::1]");
    }

    #[test]
    fn signing_string_errors_on_missing_covered_header() {
        let err = build_signing_string("GET", "/x", &headers(), &["digest"]).unwrap_err();
        assert_eq!(err, SigningError::MissingHeader("digest".to_owned()));
    }

    #[test]
    fn parses_full_signature_header() {
        let parsed = SignatureHeader::parse(
            "keyId=\"https://[2001:db8::1]/actors/mandate-x#main-key\",\
             algorithm=\"rsa-sha256\",headers=\"(request-target) host date\",signature=\"abc123==\"",
        )
        .unwrap();
        assert_eq!(
            parsed.key_id,
            "https://[2001:db8::1]/actors/mandate-x#main-key"
        );
        assert_eq!(parsed.algorithm.as_deref(), Some("rsa-sha256"));
        assert_eq!(parsed.headers, vec!["(request-target)", "host", "date"]);
        assert_eq!(parsed.signature, "abc123==");
    }

    #[test]
    fn signature_header_defaults_to_date_when_headers_absent() {
        let parsed = SignatureHeader::parse("keyId=\"k\",signature=\"s\"").unwrap();
        assert_eq!(parsed.headers, vec!["date"]);
    }

    #[test]
    fn signature_header_requires_key_and_signature() {
        assert_eq!(
            SignatureHeader::parse("algorithm=\"rsa-sha256\"").unwrap_err(),
            SignatureParseError::MissingParameter("keyId")
        );
        assert_eq!(
            SignatureHeader::parse("keyId=\"k\"").unwrap_err(),
            SignatureParseError::MissingParameter("signature")
        );
    }

    #[test]
    fn algorithm_tokens_are_stable() {
        assert_eq!(SignatureAlgorithm::RsaSha256.as_str(), "rsa-sha256");
        assert_eq!(SignatureAlgorithm::Ed25519.as_str(), "ed25519");
    }

    #[test]
    fn public_key_main_key_convention() {
        let key = PublicKey::main_key("https://[2001:db8::1]/actors/mandate-x", "PEM");
        assert_eq!(key.id, "https://[2001:db8::1]/actors/mandate-x#main-key");
        assert_eq!(key.owner, "https://[2001:db8::1]/actors/mandate-x");
    }

    /// A stub verifier proving the trait is implementable and injectable (no crypto, no network).
    struct StubVerifier {
        accept: bool,
    }
    impl SignatureVerifier for StubVerifier {
        fn verify(&self, _s: &str, _sig: &str, _k: &PublicKey) -> Result<(), VerifyError> {
            if self.accept {
                Ok(())
            } else {
                Err(VerifyError::Mismatch)
            }
        }
    }

    #[test]
    fn verifier_trait_is_injectable() {
        let key = PublicKey::main_key("https://[2001:db8::1]/actors/x", "PEM");
        let ok: &dyn SignatureVerifier = &StubVerifier { accept: true };
        let bad: &dyn SignatureVerifier = &StubVerifier { accept: false };
        assert!(ok.verify("string", "sig", &key).is_ok());
        assert_eq!(
            bad.verify("string", "sig", &key),
            Err(VerifyError::Mismatch)
        );
    }
}

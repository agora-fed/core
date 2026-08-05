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

/// Headers an inbound POST signature MUST cover (issue #10).
///
/// A signature is only as strong as what it covers. Without `(request-target)` the same
/// signature replays against any path; without `host`, against any instance; without
/// `digest`, with any body. `SignatureHeader::parse` defaults to `["date"]` when the
/// parameter is absent, so a bare `date` signature would otherwise be accepted — one
/// captured request would then be a universal forgery. This is the set Mastodon signs.
pub const REQUIRED_POST_COVERAGE: [&str; 4] = ["(request-target)", "host", "date", "digest"];

/// Clock-skew tolerance for the `Date` of an inbound signed request, in seconds.
///
/// One hour, applied symmetrically. Tight enough that a captured request stops being
/// replayable within the day; loose enough to tolerate the unsynchronised clocks that
/// are common across the fediverse. Replay INSIDE the window is separately contained by
/// the insert-before-act idempotency log, so this bound is defence in depth, not the
/// primary control — which is why it does not need to be aggressive enough to break
/// interoperability with peers whose clocks drift by minutes.
pub const MAX_DATE_SKEW_SECS: i64 = 3600;

/// Why an inbound signed request was rejected before its signature was even checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboundSignatureError {
    /// The signature does not cover a header that it must.
    UncoveredHeader(&'static str),
    /// The `Digest` header is absent.
    MissingDigest,
    /// The `Digest` header carries no `SHA-256=<base64>` component we can check.
    UnsupportedDigest,
    /// The `Digest` does not match `SHA-256(body)` — the body was substituted.
    DigestMismatch,
    /// The `Date` header is absent or not a valid HTTP date.
    UnreadableDate,
    /// The `Date` is outside the accepted skew window; carries the observed skew.
    DateOutOfWindow(i64),
}

impl std::fmt::Display for InboundSignatureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UncoveredHeader(name) => write!(f, "signature does not cover {name}"),
            Self::MissingDigest => write!(f, "missing Digest header"),
            Self::UnsupportedDigest => write!(f, "Digest header has no SHA-256 component"),
            Self::DigestMismatch => write!(f, "Digest does not match the body"),
            Self::UnreadableDate => write!(f, "missing or unparseable Date header"),
            Self::DateOutOfWindow(skew) => write!(f, "Date is {skew}s outside the skew window"),
        }
    }
}

impl std::error::Error for InboundSignatureError {}

/// Require that `covered` includes every header in [`REQUIRED_POST_COVERAGE`].
///
/// # Errors
/// Returns [`InboundSignatureError::UncoveredHeader`] naming the first missing header.
pub fn require_post_coverage(covered: &[String]) -> Result<(), InboundSignatureError> {
    for required in REQUIRED_POST_COVERAGE {
        if !covered.iter().any(|h| h.eq_ignore_ascii_case(required)) {
            return Err(InboundSignatureError::UncoveredHeader(required));
        }
    }
    Ok(())
}

/// Verify a `Digest` request header against the raw body.
///
/// Accepts the multi-value form (`SHA-512=…,SHA-256=…`) and checks the SHA-256 component,
/// ignoring any digest algorithm we do not implement. Comparison is on the DECODED bytes so
/// that padding and alphabet variations between implementations do not cause false rejects.
///
/// # Errors
/// Returns [`InboundSignatureError`] when the header is absent, carries no SHA-256 component,
/// or does not match the body.
pub fn verify_body_digest(
    digest_header: Option<&str>,
    body: &[u8],
) -> Result<(), InboundSignatureError> {
    use base64::Engine as _;
    use sha2::Digest as _;

    let header = digest_header.ok_or(InboundSignatureError::MissingDigest)?;
    let expected = sha2::Sha256::digest(body);

    let mut saw_sha256 = false;
    for part in header.split(',') {
        let Some((alg, value)) = part.trim().split_once('=') else {
            continue;
        };
        if !alg.trim().eq_ignore_ascii_case("sha-256") {
            continue;
        }
        saw_sha256 = true;
        // The value itself is base64 and may contain '=' padding, which `split_once`
        // left intact on the right-hand side. Both alphabets are tried because peers
        // differ; a decode failure is simply "not a match", never a panic.
        let raw = value.trim();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(raw)
            .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(raw));
        if decoded.map(|d| d == expected.as_slice()).unwrap_or(false) {
            return Ok(());
        }
    }

    if saw_sha256 {
        Err(InboundSignatureError::DigestMismatch)
    } else {
        Err(InboundSignatureError::UnsupportedDigest)
    }
}

/// Check that an HTTP `Date` header lies within `max_skew_secs` of `now`.
///
/// Accepts the IMF-fixdate form every fediverse peer emits
/// (`Thu, 25 Jun 2026 12:00:00 GMT`) as well as full RFC 2822 with a numeric offset.
///
/// # Errors
/// Returns [`InboundSignatureError::UnreadableDate`] when absent or unparseable, and
/// [`InboundSignatureError::DateOutOfWindow`] when it is too far from `now`.
pub fn check_date_skew(
    date_header: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
    max_skew_secs: i64,
) -> Result<(), InboundSignatureError> {
    let raw = date_header
        .map(str::trim)
        .ok_or(InboundSignatureError::UnreadableDate)?;

    let parsed = chrono::DateTime::parse_from_rfc2822(raw)
        .map(|d| d.with_timezone(&chrono::Utc))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(raw, "%a, %d %b %Y %H:%M:%S GMT")
                .map(|n| n.and_utc())
        })
        .or_else(|_| {
            // Last resort: ignore the leading weekday. Both parsers above VALIDATE it
            // against the date, so a peer that miscomputes "Tue" for a Thursday is
            // rejected outright. The weekday is redundant with the date and carries no
            // security information, so refusing delivery over it would cost
            // interoperability and buy nothing.
            let after_comma = raw.split_once(", ").map_or(raw, |(_, rest)| rest);
            chrono::NaiveDateTime::parse_from_str(after_comma, "%d %b %Y %H:%M:%S GMT")
                .map(|n| n.and_utc())
        })
        .map_err(|_| InboundSignatureError::UnreadableDate)?;

    let skew = (now - parsed).num_seconds();
    if skew.abs() > max_skew_secs {
        return Err(InboundSignatureError::DateOutOfWindow(skew));
    }
    Ok(())
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
                "Thu, 25 Jun 2026 12:00:00 GMT".to_owned(),
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
             date: Thu, 25 Jun 2026 12:00:00 GMT"
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

    // ── Inbound hardening (issue #10) ───────────────────────────────────────

    fn digest_of(body: &[u8]) -> String {
        use base64::Engine as _;
        use sha2::Digest as _;
        format!(
            "SHA-256={}",
            base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(body))
        )
    }

    fn covered(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn coverage_accepts_the_mastodon_header_set() {
        assert!(
            require_post_coverage(&covered(&["(request-target)", "host", "date", "digest"]))
                .is_ok()
        );
    }

    #[test]
    fn coverage_accepts_extra_headers_beyond_the_required_set() {
        assert!(require_post_coverage(&covered(&[
            "(request-target)",
            "host",
            "date",
            "digest",
            "content-type",
            "user-agent",
        ]))
        .is_ok());
    }

    #[test]
    fn coverage_is_case_insensitive() {
        assert!(
            require_post_coverage(&covered(&["(REQUEST-TARGET)", "Host", "DATE", "Digest"]))
                .is_ok()
        );
    }

    #[test]
    fn coverage_rejects_the_bare_date_default() {
        // `SignatureHeader::parse` defaults to this when `headers` is absent — the
        // exact shape that made one captured signature reusable anywhere.
        assert_eq!(
            require_post_coverage(&covered(&["date"])).unwrap_err(),
            InboundSignatureError::UncoveredHeader("(request-target)")
        );
    }

    #[test]
    fn coverage_rejects_each_missing_required_header() {
        let all = ["(request-target)", "host", "date", "digest"];
        for missing in all {
            let rest: Vec<&str> = all.iter().copied().filter(|h| *h != missing).collect();
            assert_eq!(
                require_post_coverage(&covered(&rest)).unwrap_err(),
                InboundSignatureError::UncoveredHeader(missing),
                "dropping {missing} must be rejected"
            );
        }
    }

    #[test]
    fn digest_accepts_the_matching_body() {
        let body = br#"{"type":"Follow"}"#;
        assert!(verify_body_digest(Some(&digest_of(body)), body).is_ok());
    }

    #[test]
    fn digest_rejects_a_substituted_body() {
        // THE ATTACK: a captured, validly-signed request whose body is swapped.
        // The signature still verifies (it covers headers, not bytes) — only the
        // digest check stands between the attacker and a forged activity.
        let signed_body = br#"{"id":"https://evil.example/1","type":"Follow"}"#;
        let swapped_body = br#"{"id":"https://evil.example/2","type":"Delete"}"#;
        assert_eq!(
            verify_body_digest(Some(&digest_of(signed_body)), swapped_body).unwrap_err(),
            InboundSignatureError::DigestMismatch
        );
    }

    #[test]
    fn digest_rejects_a_single_flipped_byte() {
        let body = b"exactly these bytes";
        let tampered = b"exactly these byteS";
        assert_eq!(
            verify_body_digest(Some(&digest_of(body)), tampered).unwrap_err(),
            InboundSignatureError::DigestMismatch
        );
    }

    #[test]
    fn digest_rejects_an_absent_header() {
        assert_eq!(
            verify_body_digest(None, b"body").unwrap_err(),
            InboundSignatureError::MissingDigest
        );
    }

    #[test]
    fn digest_reports_unsupported_when_no_sha256_component_is_present() {
        assert_eq!(
            verify_body_digest(Some("SHA-512=abc123=="), b"body").unwrap_err(),
            InboundSignatureError::UnsupportedDigest
        );
    }

    #[test]
    fn digest_finds_the_sha256_component_among_several() {
        let body = b"multi-digest body";
        let header = format!("SHA-512=ignored==,{}", digest_of(body));
        assert!(verify_body_digest(Some(&header), body).is_ok());
    }

    #[test]
    fn digest_tolerates_url_safe_base64() {
        use base64::Engine as _;
        use sha2::Digest as _;
        // A body whose SHA-256 contains bytes that encode to '+' or '/' in the
        // standard alphabet, so the two alphabets genuinely differ.
        for n in 0..64u8 {
            let body = vec![n; 8];
            let std_b64 =
                base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(&body));
            if !std_b64.contains('+') && !std_b64.contains('/') {
                continue;
            }
            let url_b64 =
                base64::engine::general_purpose::URL_SAFE.encode(sha2::Sha256::digest(&body));
            assert!(
                verify_body_digest(Some(&format!("SHA-256={url_b64}")), &body).is_ok(),
                "url-safe digest must be accepted"
            );
            return;
        }
        panic!("no body produced a distinguishing base64 encoding");
    }

    #[test]
    fn digest_does_not_panic_on_garbage() {
        for garbage in [
            "",
            "=",
            "SHA-256=",
            "SHA-256=!!!not base64!!!",
            "no-equals-sign",
        ] {
            assert!(verify_body_digest(Some(garbage), b"body").is_err());
        }
    }

    fn at(s: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(s)
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn date_accepts_imf_fixdate_inside_the_window() {
        assert!(check_date_skew(
            Some("Thu, 25 Jun 2026 12:00:00 GMT"),
            at("2026-06-25T12:00:30Z"),
            MAX_DATE_SKEW_SECS
        )
        .is_ok());
    }

    #[test]
    fn date_accepts_rfc2822_with_a_numeric_offset() {
        assert!(check_date_skew(
            Some("Thu, 25 Jun 2026 09:00:00 -0300"),
            at("2026-06-25T12:00:00Z"),
            MAX_DATE_SKEW_SECS
        )
        .is_ok());
    }

    #[test]
    fn date_rejects_a_stale_capture() {
        // A request captured yesterday and replayed today.
        let err = check_date_skew(
            Some("Wed, 24 Jun 2026 12:00:00 GMT"),
            at("2026-06-25T12:00:00Z"),
            MAX_DATE_SKEW_SECS,
        )
        .unwrap_err();
        assert_eq!(err, InboundSignatureError::DateOutOfWindow(86_400));
    }

    #[test]
    fn date_rejects_the_future_as_well_as_the_past() {
        let err = check_date_skew(
            Some("Thu, 25 Jun 2026 14:00:00 GMT"),
            at("2026-06-25T12:00:00Z"),
            MAX_DATE_SKEW_SECS,
        )
        .unwrap_err();
        assert_eq!(err, InboundSignatureError::DateOutOfWindow(-7_200));
    }

    #[test]
    fn date_window_boundary_is_inclusive() {
        // Exactly at the limit is accepted; one second beyond is not.
        assert!(check_date_skew(
            Some("Thu, 25 Jun 2026 11:00:00 GMT"),
            at("2026-06-25T12:00:00Z"),
            MAX_DATE_SKEW_SECS
        )
        .is_ok());
        assert!(check_date_skew(
            Some("Thu, 25 Jun 2026 10:59:59 GMT"),
            at("2026-06-25T12:00:00Z"),
            MAX_DATE_SKEW_SECS
        )
        .is_err());
    }

    #[test]
    fn date_rejects_absent_or_unparseable_values() {
        assert_eq!(
            check_date_skew(None, at("2026-06-25T12:00:00Z"), MAX_DATE_SKEW_SECS).unwrap_err(),
            InboundSignatureError::UnreadableDate
        );
        for bad in ["", "yesterday", "2026-06-25T12:00:00Z", "Tue, 99 Xxx 2026"] {
            assert_eq!(
                check_date_skew(Some(bad), at("2026-06-25T12:00:00Z"), MAX_DATE_SKEW_SECS)
                    .unwrap_err(),
                InboundSignatureError::UnreadableDate,
                "{bad:?} must not parse"
            );
        }
    }

    #[test]
    fn inbound_errors_render_distinct_messages() {
        // These strings reach the logs an operator reads during an incident.
        let rendered: Vec<String> = vec![
            InboundSignatureError::UncoveredHeader("digest").to_string(),
            InboundSignatureError::MissingDigest.to_string(),
            InboundSignatureError::UnsupportedDigest.to_string(),
            InboundSignatureError::DigestMismatch.to_string(),
            InboundSignatureError::UnreadableDate.to_string(),
            InboundSignatureError::DateOutOfWindow(42).to_string(),
        ];
        let unique: std::collections::HashSet<&String> = rendered.iter().collect();
        assert_eq!(unique.len(), rendered.len(), "messages must be distinct");
        assert!(rendered[5].contains("42"));
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

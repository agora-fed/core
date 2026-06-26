//! Concrete RSA-SHA256 cryptographic backend for HTTP Signatures (ADR-0010 W2.2).
//!
//! Implements the `SignatureVerifier` trait (verifying inbound signatures) and a pure `sign`
//! function (producing outbound signatures). Both operate on the canonical signing string from
//! `crate::signatures::build_signing_string` — they perform no I/O and contain no live network
//! work, keeping the federation crate Tier-3.
//!
//! ## Algorithm
//! - RSASSA-PKCS1-v1_5 with SHA-256 — the fediverse default (`algorithm="rsa-sha256"`).
//! - Public keys are PEM SubjectPublicKeyInfo; private keys are PEM PKCS#8 (matching `keys.rs`).
//! - Signatures are base64-encoded raw bytes (standard alphabet, with `=` padding) — the format
//!   Mastodon and friends emit and expect.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey};
use rsa::pkcs1v15::SigningKey;
use rsa::sha2::Sha256;
use rsa::signature::{SignatureEncoding, Signer};
use rsa::{Pkcs1v15Sign, RsaPrivateKey, RsaPublicKey};
use sha2::Digest;

use crate::signatures::{PublicKey, SignatureVerifier, VerifyError};

/// Concrete verifier: RSA-PKCS1v15 over SHA-256 of the canonical signing string. Stateless;
/// holds no keys. Instantiate once at the gateway and inject.
#[derive(Debug, Default, Clone, Copy)]
pub struct RsaSha256Verifier;

impl SignatureVerifier for RsaSha256Verifier {
    fn verify(
        &self,
        signing_string: &str,
        signature_b64: &str,
        key: &PublicKey,
    ) -> Result<(), VerifyError> {
        let pub_key = RsaPublicKey::from_public_key_pem(&key.public_key_pem)
            .map_err(|_| VerifyError::Unsupported)?;
        let sig_bytes = STANDARD
            .decode(signature_b64)
            .map_err(|_| VerifyError::Mismatch)?;
        // Spec: signers hash the signing string with SHA-256 first, then sign the hash.
        let digest = Sha256::digest(signing_string.as_bytes());
        pub_key
            .verify(Pkcs1v15Sign::new::<Sha256>(), &digest, &sig_bytes)
            .map_err(|_| VerifyError::Mismatch)
    }
}

/// Sign a canonical signing string with the given PKCS#8 RSA private PEM, producing the
/// base64-encoded signature for the `signature="..."` parameter of an outbound `Signature` header.
///
/// CPU-bound (~1 ms on modern x86); call from a blocking task if the request hot path matters
/// — for typical outbound delivery (10s of follows) the cost is invisible.
///
/// # Errors
/// Returns the underlying `rsa::Error` when the PEM is malformed or signing fails (effectively
/// only on a corrupt private_pem column, which is a bug, not a runtime condition).
pub fn sign_with_pem(private_pem: &str, signing_string: &str) -> Result<String, rsa::Error> {
    let pk = RsaPrivateKey::from_pkcs8_pem(private_pem).map_err(|_| rsa::Error::Internal)?;
    let signer = SigningKey::<Sha256>::new(pk);
    let sig = signer.sign(signing_string.as_bytes());
    Ok(STANDARD.encode(sig.to_bytes()))
}

/// Build the `Signature` header value for an outbound request, given the already-built
/// canonical signing string. The header lists the covered headers in the SAME ORDER the caller
/// passed to `build_signing_string`, which the receiving verifier replays verbatim.
#[must_use]
pub fn signature_header_value(key_id: &str, covered: &[&str], signature_b64: &str) -> String {
    let headers_param = covered.join(" ");
    format!(
        "keyId=\"{key_id}\",algorithm=\"rsa-sha256\",headers=\"{headers_param}\",signature=\"{signature_b64}\""
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::keys::generate_actor_keypair;
    use crate::signatures::build_signing_string;

    #[test]
    fn sign_and_verify_round_trip() {
        let kp = generate_actor_keypair().unwrap();
        let signing_string = "(request-target): post /inbox\nhost: example.test";
        let sig = sign_with_pem(&kp.private_pem, signing_string).unwrap();
        let key = PublicKey::main_key("https://example.test/actors/x", kp.public_pem);
        RsaSha256Verifier
            .verify(signing_string, &sig, &key)
            .expect("signature must verify");
    }

    #[test]
    fn tampered_signing_string_fails_verification() {
        let kp = generate_actor_keypair().unwrap();
        let sig = sign_with_pem(&kp.private_pem, "original").unwrap();
        let key = PublicKey::main_key("https://example.test/actors/x", kp.public_pem);
        assert_eq!(
            RsaSha256Verifier.verify("TAMPERED", &sig, &key),
            Err(VerifyError::Mismatch)
        );
    }

    #[test]
    fn header_value_matches_mastodons_format() {
        let value = signature_header_value(
            "https://example.test/actors/x#main-key",
            &["(request-target)", "host", "date"],
            "abc123==",
        );
        assert_eq!(
            value,
            r#"keyId="https://example.test/actors/x#main-key",algorithm="rsa-sha256",headers="(request-target) host date",signature="abc123==""#
        );
    }

    #[test]
    fn full_signing_string_round_trip_using_real_builder() {
        let kp = generate_actor_keypair().unwrap();
        let headers = vec![
            ("Host".to_owned(), "example.test".to_owned()),
            ("Date".to_owned(), "Tue, 25 Jun 2026 12:00:00 GMT".to_owned()),
        ];
        let ss = build_signing_string(
            "POST",
            "/users/x/inbox",
            &headers,
            &["(request-target)", "host", "date"],
        )
        .unwrap();
        let sig = sign_with_pem(&kp.private_pem, &ss).unwrap();
        let key = PublicKey::main_key("https://example.test/actors/x", kp.public_pem);
        RsaSha256Verifier.verify(&ss, &sig, &key).unwrap();
    }
}

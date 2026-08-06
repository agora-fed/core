//! Encryption at rest for citizens' identifiers (issue #15, migration 0682).
//!
//! CPF, voter registration, phone and the TOTP shared secret used to sit in cleartext
//! in the main tables, beside the password hash. A database dump handed all of them
//! over — and the TOTP secret in particular is a permanent 2FA bypass until it is
//! rotated, which nobody would know to do.
//!
//! The repo already encrypted VENDOR credentials this way (`intercoms_provider_config`,
//! 0660). This applies the same standard to the people's own identifiers.
//!
//! **Two mechanisms, because the columns are asked different questions:**
//!
//! * **Encryption** (`pgp_sym_encrypt`, key held outside the database) for anything
//!   that must be readable again.
//! * **A keyed HMAC** for CPF, which is only ever asked "is this one already
//!   registered?". A blind index answers that without holding the answer. Note that
//!   the CPF is WRITE-ONLY in this codebase — nothing reads it back — so storing only
//!   the HMAC and no ciphertext at all would be strictly safer. That is a deliberate
//!   product decision (it is irreversible), recorded on #15 rather than taken here.
//!
//! **Fail closed.** With no key configured, writing an identifier ERRORS instead of
//! silently falling back to cleartext. A fallback is how a column ends up half
//! encrypted and everyone believes otherwise.

use hmac::{Hmac, Mac};
use sha2::Sha256;

/// Why an identifier could not be protected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PiiError {
    /// `PII_ENCRYPTION_KEY` is unset or empty.
    NoKey,
}

impl std::fmt::Display for PiiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoKey => write!(
                f,
                "PII_ENCRYPTION_KEY is not configured; refusing to store an identifier in the clear"
            ),
        }
    }
}

impl std::error::Error for PiiError {}

/// The key that protects identifiers at rest, from the environment.
///
/// # Errors
/// [`PiiError::NoKey`] when unset or blank.
pub fn key() -> Result<String, PiiError> {
    match std::env::var("PII_ENCRYPTION_KEY") {
        Ok(k) if !k.trim().is_empty() => Ok(k),
        _ => Err(PiiError::NoKey),
    }
}

/// Is protection configured? Used by callers that must degrade rather than fail.
#[must_use]
pub fn is_configured() -> bool {
    key().is_ok()
}

/// Digits only, so formatting differences never produce two indexes for one identifier.
///
/// `123.456.789-09` and `12345678909` are the same CPF; without normalising, the same
/// person could register twice and the uniqueness index would be none the wiser.
#[must_use]
pub fn normalize_digits(raw: &str) -> String {
    raw.chars().filter(char::is_ascii_digit).collect()
}

/// Keyed blind index over an identifier: lets a UNIQUE constraint work on a value the
/// database never holds.
///
/// HMAC rather than a plain hash: a bare SHA-256 of a CPF is trivially reversible by
/// enumeration — the space is 10^11 with a check digit, which is nothing. The key is
/// what makes the index useless to someone holding only the dump.
///
/// The key is a PARAMETER, not read from the environment in here. Reading it inside
/// made this a function of hidden global state, and its tests raced each other over
/// that variable — green locally, red in CI. A pure function has no such failure mode.
///
/// # Errors
/// [`PiiError::NoKey`] when `key` is blank.
pub fn blind_index_with(key: &str, raw: &str) -> Result<Vec<u8>, PiiError> {
    if key.trim().is_empty() {
        return Err(PiiError::NoKey);
    }
    let normalized = normalize_digits(raw);
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).map_err(|_| PiiError::NoKey)?;
    mac.update(normalized.as_bytes());
    Ok(mac.finalize().into_bytes().to_vec())
}

/// [`blind_index_with`] using the configured key.
///
/// # Errors
/// [`PiiError::NoKey`] when no key is configured.
pub fn blind_index(raw: &str) -> Result<Vec<u8>, PiiError> {
    blind_index_with(&key()?, raw)
}

/// The last four digits, which is what the API already shows. Kept in cleartext on
/// purpose: it is public at the edge, and deriving it from ciphertext on every read
/// would mean decrypting to render a mask.
#[must_use]
pub fn last4(raw: &str) -> Option<String> {
    let d = normalize_digits(raw);
    (d.len() >= 4).then(|| d[d.len() - 4..].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const K: &str = "chave-de-teste";

    #[test]
    fn formatting_does_not_change_the_index() {
        assert_eq!(
            blind_index_with(K, "123.456.789-09").unwrap(),
            blind_index_with(K, "12345678909").unwrap(),
            "the same CPF must index once, however it was typed"
        );
    }

    #[test]
    fn different_identifiers_index_differently() {
        assert_ne!(
            blind_index_with(K, "12345678909").unwrap(),
            blind_index_with(K, "98765432100").unwrap()
        );
    }

    #[test]
    fn the_index_depends_on_the_key() {
        // Whoever holds the dump but not the key cannot rebuild the column: this is
        // the whole reason it is an HMAC and not a bare hash.
        assert_ne!(
            blind_index_with("chave-a", "12345678909").unwrap(),
            blind_index_with("chave-b", "12345678909").unwrap()
        );
    }

    #[test]
    fn the_index_is_not_the_value() {
        let idx = blind_index_with(K, "12345678909").unwrap();
        assert_eq!(idx.len(), 32);
        assert!(!String::from_utf8_lossy(&idx).contains("12345678909"));
    }

    #[test]
    fn a_blank_key_refuses_rather_than_degrading() {
        // The failure mode this guards against is a column that is half protected
        // while everyone believes otherwise.
        for blank in ["", "   "] {
            assert_eq!(
                blind_index_with(blank, "12345678909").unwrap_err(),
                PiiError::NoKey
            );
        }
    }

    #[test]
    fn last4_reads_the_digits_not_the_characters() {
        assert_eq!(last4("123.456.789-09").as_deref(), Some("8909"));
        assert_eq!(last4("0123 4567 8901").as_deref(), Some("8901"));
        assert_eq!(last4("12").as_deref(), None, "too short to mask");
        assert_eq!(last4("").as_deref(), None);
    }

    #[test]
    fn normalisation_keeps_only_digits() {
        assert_eq!(normalize_digits("(11) 98765-4321"), "11987654321");
        assert_eq!(normalize_digits("abc"), "");
    }
}

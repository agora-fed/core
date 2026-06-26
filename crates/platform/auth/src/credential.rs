//! Sovereign credential identity: e-mail + senha (Argon2id) + **CPF**. Auth is verified by CPF
//! (not an external IdP). CPF check-digits are validated algorithmically here (offline, free); a
//! pluggable [`CpfVerifier`] allows confirming against an official source (Serpro/KYC) later to
//! reach the `verified` status. See ADR-0008.

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;

use crate::Error;

/// A normalized, check-digit-valid Brazilian CPF (11 digits, no punctuation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cpf(String);

impl Cpf {
    /// Parse and validate a CPF: strips punctuation, requires 11 digits, rejects repeated-digit
    /// sequences, and verifies both check digits.
    ///
    /// # Errors
    /// [`Error::Validation`] if the CPF is malformed or its check digits are wrong.
    pub fn parse(raw: &str) -> Result<Self, Error> {
        let digits: Vec<u8> = raw
            .chars()
            .filter_map(|c| c.to_digit(10).map(|d| d as u8))
            .collect();
        if digits.len() != 11 {
            return Err(Error::Validation("CPF deve ter 11 dígitos".to_string()));
        }
        if digits.iter().all(|&d| d == digits[0]) {
            return Err(Error::Validation("CPF inválido".to_string()));
        }
        if check_digit(&digits[..9], 10) != digits[9]
            || check_digit(&digits[..10], 11) != digits[10]
        {
            return Err(Error::Validation(
                "CPF inválido (dígitos verificadores)".to_string(),
            ));
        }
        Ok(Self(digits.iter().map(|d| char::from(b'0' + d)).collect()))
    }

    /// The 11-digit normalized string (storage form).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Compute a CPF check digit over `slice` with the given starting weight.
fn check_digit(slice: &[u8], start_weight: u32) -> u8 {
    let sum: u32 = slice
        .iter()
        .enumerate()
        .map(|(i, &d)| u32::from(d) * (start_weight - i as u32))
        .sum();
    let rem = sum % 11;
    if rem < 2 {
        0
    } else {
        (11 - rem) as u8
    }
}

/// Assurance status of a citizen's CPF.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpfStatus {
    /// Not yet checked.
    Unverified,
    /// Check digits valid (algorithmic).
    Validated,
    /// Confirmed against an official source (Serpro/KYC).
    Verified,
}

impl CpfStatus {
    /// Database string form.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            CpfStatus::Unverified => "unverified",
            CpfStatus::Validated => "validated",
            CpfStatus::Verified => "verified",
        }
    }
}

/// Pluggable CPF verification. The algorithmic impl confirms only the check digits; a future
/// Serpro/KYC impl confirms the CPF is real and belongs to the holder (raising to `Verified`).
#[async_trait::async_trait]
pub trait CpfVerifier: Send + Sync {
    /// Verify a (already check-digit-valid) CPF, returning its assurance status.
    async fn verify(&self, cpf: &Cpf) -> CpfStatus;
}

/// Offline verifier: a parsed [`Cpf`] already passed the check digits, so this returns `Validated`.
#[derive(Debug, Clone, Copy, Default)]
pub struct AlgorithmicCpfVerifier;

#[async_trait::async_trait]
impl CpfVerifier for AlgorithmicCpfVerifier {
    async fn verify(&self, _cpf: &Cpf) -> CpfStatus {
        CpfStatus::Validated
    }
}

/// Hash a password with Argon2id (PHC string). Never store the plaintext.
///
/// # Errors
/// [`Error::Validation`] on an empty/too-short password; otherwise wraps the hashing failure.
pub fn hash_password(password: &str) -> Result<String, Error> {
    if password.len() < 8 {
        return Err(Error::Validation(
            "senha deve ter ao menos 8 caracteres".to_string(),
        ));
    }
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| Error::Dependency {
            dependency: "argon2",
            source: Box::new(std::io::Error::other(e.to_string())),
        })
}

/// Verify a password against a stored Argon2id PHC hash. Returns `false` on any mismatch.
#[must_use]
pub fn verify_password(password: &str, phc_hash: &str) -> bool {
    PasswordHash::new(phc_hash)
        .map(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_cpf_parses_and_normalizes() {
        // A well-known valid test CPF.
        let cpf = Cpf::parse("529.982.247-25").expect("valid");
        assert_eq!(cpf.as_str(), "52998224725");
    }

    #[test]
    fn invalid_check_digits_rejected() {
        assert!(Cpf::parse("529.982.247-24").is_err());
    }

    #[test]
    fn repeated_digits_rejected() {
        assert!(Cpf::parse("111.111.111-11").is_err());
        assert!(Cpf::parse("000.000.000-00").is_err());
    }

    #[test]
    fn wrong_length_rejected() {
        assert!(Cpf::parse("123").is_err());
    }

    #[test]
    fn password_hash_roundtrips_and_rejects_wrong() {
        let hash = hash_password("correct horse battery").expect("hash");
        assert!(verify_password("correct horse battery", &hash));
        assert!(!verify_password("wrong password", &hash));
        assert!(!verify_password("x", "not-a-phc-hash"));
    }

    #[test]
    fn short_password_rejected() {
        assert!(hash_password("short").is_err());
    }

    #[tokio::test]
    async fn algorithmic_verifier_returns_validated() {
        let cpf = Cpf::parse("52998224725").unwrap();
        assert_eq!(
            AlgorithmicCpfVerifier.verify(&cpf).await,
            CpfStatus::Validated
        );
    }
}

//! Sovereign credential identity: e-mail + senha (Argon2id) + **CPF**. Auth is verified by CPF
//! (not an external IdP). See ADR-0008.
//!
//! **ADR-0015:** the CPF (the Brazilian identity document) is Brazil-specific code and was
//! moved behind the localization boundary in [`dsoc_l10n_br::document`]. We re-export
//! [`Cpf`], [`CpfStatus`], [`CpfVerifier`] and [`AlgorithmicCpfVerifier`] here to preserve the
//! `crate::credential::*` paths used by the rest of the crate. The password (Argon2id) is
//! country-agnostic and still lives here.

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;

use crate::Error;

// CPF — the Brazilian identity document (l10n_br, ADR-0015). The logic (check digits,
// status, pluggable verifier) lives in `dsoc_l10n_br::document`; here we only re-export.
pub use dsoc_l10n_br::document::{AlgorithmicCpfVerifier, Cpf, CpfStatus, CpfVerifier};

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

    #[test]
    fn cpf_reexport_still_parses() {
        // Proof that the l10n_br re-export keeps the `crate::credential::Cpf` path working.
        let cpf = Cpf::parse("529.982.247-25").expect("valid");
        assert_eq!(cpf.as_str(), "52998224725");
    }
}

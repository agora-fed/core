//! Sovereign credential identity: e-mail + senha (Argon2id) + **CPF**. Auth is verified by CPF
//! (not an external IdP). See ADR-0008.
//!
//! **ADR-0015:** o CPF (documento de identidade brasileiro) é código Brasil-específico e foi
//! movido para trás da fronteira de localização em [`dsoc_l10n_br::document`]. Reexportamos aqui
//! [`Cpf`], [`CpfStatus`], [`CpfVerifier`] e [`AlgorithmicCpfVerifier`] para preservar os caminhos
//! `crate::credential::*` usados pelo resto do crate. A senha (Argon2id) é agnóstica de país e
//! continua morando aqui.

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;

use crate::Error;

// CPF — documento de identidade brasileiro (l10n_br, ADR-0015). A lógica (dígitos verificadores,
// status, verificador plugável) vive em `dsoc_l10n_br::document`; aqui só reexportamos.
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
        // Prova de que o reexport de l10n_br mantém o caminho `crate::credential::Cpf`.
        let cpf = Cpf::parse("529.982.247-25").expect("valid");
        assert_eq!(cpf.as_str(), "52998224725");
    }
}

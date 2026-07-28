//! Documento de identidade brasileiro — **CPF** (ADR-0015). Movido de `dsoc-auth::credential`.
//!
//! O CPF é verificado algoritmicamente aqui (offline, de graça): dígitos verificadores. A
//! confirmação contra a fonte oficial (base autorizada via SaaS cpf-verify) vive em
//! [`crate::saas`] e é elevada à abstração agnóstica [`dsoc_core::IdentityVerifier`] em
//! [`crate::identity`]. Veja ADR-0008 e ADR-0015.

use dsoc_core::{Error, IdentityAssurance};

/// Um CPF brasileiro normalizado e com dígitos verificadores válidos (11 dígitos, sem pontuação).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cpf(String);

impl Cpf {
    /// Faz o parse e valida um CPF: tira a pontuação, exige 11 dígitos, rejeita sequências de
    /// dígito repetido e confere os dois dígitos verificadores.
    ///
    /// # Errors
    /// [`Error::Validation`] se o CPF estiver malformado ou com DV errado.
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

    /// A string normalizada de 11 dígitos (forma de armazenamento).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Calcula um dígito verificador de CPF sobre `slice` com o peso inicial dado.
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

/// Nível de garantia do CPF de um cidadão. Espelho BR do agnóstico [`IdentityAssurance`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpfStatus {
    /// Ainda não checado.
    Unverified,
    /// Dígitos verificadores válidos (algorítmico).
    Validated,
    /// Confirmado contra fonte oficial (Serpro/KYC).
    Verified,
}

impl CpfStatus {
    /// Forma string do banco.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            CpfStatus::Unverified => "unverified",
            CpfStatus::Validated => "validated",
            CpfStatus::Verified => "verified",
        }
    }
}

impl From<CpfStatus> for IdentityAssurance {
    fn from(s: CpfStatus) -> Self {
        match s {
            CpfStatus::Unverified => IdentityAssurance::Unverified,
            CpfStatus::Validated => IdentityAssurance::Validated,
            CpfStatus::Verified => IdentityAssurance::Verified,
        }
    }
}

impl From<IdentityAssurance> for CpfStatus {
    fn from(a: IdentityAssurance) -> Self {
        match a {
            IdentityAssurance::Unverified => CpfStatus::Unverified,
            IdentityAssurance::Validated => CpfStatus::Validated,
            IdentityAssurance::Verified => CpfStatus::Verified,
        }
    }
}

/// Verificação de CPF plugável. A impl algorítmica confirma só os dígitos verificadores; uma
/// futura impl Serpro/KYC confirma que o CPF é real e pertence ao portador (subindo a `Verified`).
#[async_trait::async_trait]
pub trait CpfVerifier: Send + Sync {
    /// Verifica um CPF (já com DV válido) e devolve seu nível de garantia.
    async fn verify(&self, cpf: &Cpf) -> CpfStatus;
}

/// Verificador offline: um [`Cpf`] já passou pelos DVs, então devolve `Validated`.
#[derive(Debug, Clone, Copy, Default)]
pub struct AlgorithmicCpfVerifier;

#[async_trait::async_trait]
impl CpfVerifier for AlgorithmicCpfVerifier {
    async fn verify(&self, _cpf: &Cpf) -> CpfStatus {
        CpfStatus::Validated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_cpf_parses_and_normalizes() {
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
    fn status_maps_to_core_assurance() {
        assert_eq!(
            IdentityAssurance::from(CpfStatus::Validated),
            IdentityAssurance::Validated
        );
        assert_eq!(
            CpfStatus::from(IdentityAssurance::Verified),
            CpfStatus::Verified
        );
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

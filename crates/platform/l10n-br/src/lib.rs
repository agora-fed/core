//! # dsoc-l10n-br — localização brasileira (l10n_br)
//!
//! Módulo de localização do Brasil no estilo Odoo (`l10n_br`), conforme ADR-0015. Empacota tudo
//! que é específico do Brasil **por trás das abstrações agnósticas de país do [`dsoc_core::l10n`]**:
//!
//! - [`document`] — **CPF** (documento de identidade): dígitos verificadores + status.
//! - [`saas`] — cliente do SaaS `cpf-verify` (base autorizada; R-KYC).
//! - [`identity`] — [`BrIdentityVerifier`], impl de [`dsoc_core::IdentityVerifier`].
//! - [`territorial`] — [`BrTerritorialProvider`] (IBGE), impl de [`dsoc_core::TerritorialProvider`].
//! - [`voter`] — [`BrVoterRegistration`] (Título de Eleitor), impl de [`dsoc_core::VoterRegistration`].
//!
//! O core só fala com os traits; esta crate é a 1ª implementação concreta. Outras instalações
//! plugam seu próprio `l10n_<cc>` sem tocar o núcleo.

#![forbid(unsafe_code)]

pub mod document;
pub mod identity;
pub mod saas;
pub mod territorial;
pub mod voter;

pub use document::{AlgorithmicCpfVerifier, Cpf, CpfStatus, CpfVerifier};
pub use identity::BrIdentityVerifier;
pub use territorial::BrTerritorialProvider;
pub use voter::BrVoterRegistration;

use dsoc_core::{Localization, VoterRegistration};

/// Código ISO-3166-1 alfa-2 do país desta localização.
pub const COUNTRY_CODE: &str = "BR";

/// A localização brasileira ativa (Pindorama → `l10n_br`). Agrupa os provedores agnósticos numa
/// única [`Localization`] resolvida do código do país configurado na instalação.
#[derive(Debug, Clone, Copy, Default)]
pub struct BrLocalization {
    voter: BrVoterRegistration,
}

impl BrLocalization {
    /// Cria a localização brasileira.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl Localization for BrLocalization {
    fn country_code(&self) -> &'static str {
        COUNTRY_CODE
    }

    fn voter_registration(&self) -> Option<&dyn VoterRegistration> {
        Some(&self.voter)
    }
}

/// Resolve a localização a partir do código do país da instalação (case-insensitive). Devolve
/// `None` pra países ainda sem módulo `l10n_<cc>`. Hoje só o Brasil está implementado; o wiring do
/// gateway usa `BR` por default (Pindorama).
#[must_use]
pub fn resolve(country_code: &str) -> Option<BrLocalization> {
    if country_code.eq_ignore_ascii_case(COUNTRY_CODE) {
        Some(BrLocalization::new())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_br_case_insensitively() {
        assert!(resolve("BR").is_some());
        assert!(resolve("br").is_some());
        assert!(resolve("FR").is_none());
    }

    #[test]
    fn br_localization_exposes_voter_registration() {
        let l = BrLocalization::new();
        assert_eq!(l.country_code(), "BR");
        let voter = l.voter_registration().expect("BR tem título");
        assert_eq!(voter.validate("0000-0001-0396").unwrap(), "000000010396");
    }
}

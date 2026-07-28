//! `BrIdentityVerifier` — a implementação BR do trait agnóstico [`dsoc_core::IdentityVerifier`]
//! (ADR-0015). É a ponte que eleva o SaaS cpf-verify (BR, [`crate::saas`]) à abstração do core:
//! traduz a [`dsoc_core::IdentityCheck`] agnóstica pra [`crate::saas::IdentityQuery`] (campos BR:
//! `cpf`/`nome`/…) e o veredito em faixas de volta pra [`dsoc_core::IdentityOutcome`].

use std::sync::Arc;

use dsoc_core::{IdentityCheck, IdentityOutcome, IdentityVerifier};

use crate::saas::{self, IdentityQuery};

/// Verificador de identidade brasileiro. Encapsula um verificador SaaS ([`saas::IdentityVerifier`])
/// — por padrão resolvido de `CPF_VERIFY_URL` via [`saas::from_env`], ou um Noop em ambientes sem
/// o serviço (fail-open). O CPF já chega com os dígitos verificadores validados (ver
/// [`crate::document::Cpf`]); aqui é o confronto contra a base autorizada.
pub struct BrIdentityVerifier {
    inner: Arc<dyn saas::IdentityVerifier>,
}

impl std::fmt::Debug for BrIdentityVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrIdentityVerifier").finish_non_exhaustive()
    }
}

impl BrIdentityVerifier {
    /// Constrói envolvendo um verificador SaaS já resolvido.
    #[must_use]
    pub fn new(inner: Arc<dyn saas::IdentityVerifier>) -> Self {
        Self { inner }
    }

    /// Resolve o verificador da configuração de ambiente (HTTP se `CPF_VERIFY_URL`, senão Noop).
    #[must_use]
    pub fn from_env() -> Self {
        Self::new(saas::from_env())
    }
}

#[async_trait::async_trait]
impl IdentityVerifier for BrIdentityVerifier {
    async fn verify_identity(&self, check: &IdentityCheck) -> IdentityOutcome {
        let query = IdentityQuery {
            cpf: check.document_id.clone(),
            nome: check.full_name.clone(),
            nascimento: check.birth_date.clone(),
            sexo: check.sex.clone(),
        };
        let verdict = self.inner.verify_identity(&query).await;
        IdentityOutcome {
            found: verdict.found,
            band: verdict.faixa().into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsoc_core::IdentityBand;

    #[tokio::test]
    async fn noop_bridge_is_skipped_and_fails_open() {
        let v = BrIdentityVerifier::new(Arc::new(saas::NoopIdentityVerifier));
        let check = IdentityCheck {
            document_id: "52998224725".to_owned(),
            full_name: "Fulano de Tal".to_owned(),
            birth_date: None,
            sex: None,
        };
        let outcome = v.verify_identity(&check).await;
        assert_eq!(outcome.band, IdentityBand::Skipped);
        assert!(outcome.allows_registration());
    }
}

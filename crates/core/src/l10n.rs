//! Localização (l10n): as abstrações **agnósticas de país** do core (ADR-0015, estilo Odoo
//! `l10n_br`). O núcleo do ÁGORA não conhece CPF, Título de Eleitor ou IBGE — só conhece os
//! três traits deste módulo. Cada país pluga um módulo `l10n_<cc>` (ex.: `dsoc-l10n-br`) que os
//! implementa. Identificadores em inglês (ADR-0013); a cópia de UI específica fica na localização.
//!
//! - [`IdentityVerifier`]   — confronta um **documento de identidade** (CPF, SSN, DNI, …).
//! - [`TerritorialProvider`] — hierarquia **país → estado → município** (eixo de escopo de
//!   sorteio/federação/campanha).
//! - [`VoterRegistration`]  — conceito **opcional** de registro eleitoral (Título, etc.).

use crate::error::Result;

// ---------------------------------------------------------------------------
// IdentityVerifier — documento de identidade
// ---------------------------------------------------------------------------

/// Nível de garantia de um documento de identidade — do menos ao mais forte.
///
/// Mapeia diretamente o antigo `CpfStatus` do `l10n_br`, mas em termos neutros: `Validated` é a
/// checagem algorítmica (dígitos verificadores), `Verified` é a confirmação contra uma fonte
/// oficial do país.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IdentityAssurance {
    /// Ainda não checado.
    Unverified,
    /// Válido algoritmicamente (dígitos verificadores).
    Validated,
    /// Confirmado contra a fonte oficial do país (KYC/registro).
    Verified,
}

impl IdentityAssurance {
    /// Forma estável pra persistência/auditoria (compatível com o schema atual: `unverified` /
    /// `validated` / `verified`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            IdentityAssurance::Unverified => "unverified",
            IdentityAssurance::Validated => "validated",
            IdentityAssurance::Verified => "verified",
        }
    }
}

/// Faixa de confiança de um confronto de identidade contra a base autorizada do país (não é
/// probabilidade, é faixa calibrada). Neutra: cada localização mapeia sua nomenclatura de serviço
/// (ex.: `ACEITA`/`REVISA`/`ESCALA`/`REJEITA` no BR) pra estas variantes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityBand {
    /// Bate com folga — identidade forte.
    Accept,
    /// Bate, mas merece revisão humana.
    Review,
    /// Sinal ambíguo — escalar.
    Escalate,
    /// Não bate — bloquear.
    Reject,
    /// Verificação não executada (serviço ausente/fora) — degradação graciosa.
    Skipped,
}

/// O que se envia pra confrontar um documento com a base autorizada (agnóstico de país).
#[derive(Debug, Clone)]
pub struct IdentityCheck {
    /// Número do documento normalizado (CPF, SSN, DNI, …), sem pontuação.
    pub document_id: String,
    /// Nome completo informado pela pessoa.
    pub full_name: String,
    /// Data de nascimento `YYYY-MM-DD`, se informada.
    pub birth_date: Option<String>,
    /// Sexo (`M`/`F`), se informado.
    pub sex: Option<String>,
}

/// Veredito verdict-only de um confronto de identidade (nunca devolve o dado armazenado).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IdentityOutcome {
    /// A base encontrou o documento?
    pub found: bool,
    /// A faixa calibrada do confronto.
    pub band: IdentityBand,
}

impl IdentityOutcome {
    /// Veredito de "não verificado" — usado na degradação graciosa (fail-open).
    #[must_use]
    pub fn skipped() -> Self {
        Self {
            found: false,
            band: IdentityBand::Skipped,
        }
    }

    /// Cadastro pode prosseguir? Tudo menos `Reject` segue (Skipped inclusive — fail-open).
    #[must_use]
    pub fn allows_registration(&self) -> bool {
        !matches!(self.band, IdentityBand::Reject)
    }

    /// Identidade fortemente confirmada (`Accept`) → candidata a `IdentityAssurance::Verified`.
    #[must_use]
    pub fn is_strong(&self) -> bool {
        matches!(self.band, IdentityBand::Accept)
    }

    /// Precisa de revisão humana (`Review`/`Escalate`).
    #[must_use]
    pub fn needs_review(&self) -> bool {
        matches!(self.band, IdentityBand::Review | IdentityBand::Escalate)
    }
}

/// Porta plugável de verificação de **documento de identidade** do país. A localização pluga a
/// implementação (BR = CPF: dígitos verificadores + SaaS cpf-verify). Nunca entra em pânico; erro
/// de transporte vira [`IdentityOutcome::skipped`].
#[async_trait::async_trait]
pub trait IdentityVerifier: Send + Sync {
    /// Confronta a consulta com a base autorizada do país.
    async fn verify_identity(&self, check: &IdentityCheck) -> IdentityOutcome;
}

// ---------------------------------------------------------------------------
// TerritorialProvider — país → estado → município
// ---------------------------------------------------------------------------

/// Um município (unidade territorial de base). Neutro: no BR o `code` é o código IBGE.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Municipality {
    /// Código estável da unidade (IBGE no BR).
    pub code: i32,
    /// Nome apresentável.
    pub name: String,
}

/// Porta plugável da hierarquia territorial do país. A localização pluga a fonte (BR = tabela
/// `municipio_ibge`). É o eixo de escopo de sorteio/federação/campanha (ADR-0014).
#[async_trait::async_trait]
pub trait TerritorialProvider: Send + Sync {
    /// Municípios de uma subdivisão de 1º nível (UF no BR), ordenados por nome.
    ///
    /// # Errors
    /// [`crate::Error::Storage`] em falha de persistência.
    async fn municipalities(&self, subdivision: &str) -> Result<Vec<Municipality>>;

    /// O município `code` pertence à subdivisão `subdivision`? (Validação de domicílio.)
    ///
    /// # Errors
    /// [`crate::Error::Storage`] em falha de persistência.
    async fn municipality_in_subdivision(&self, code: i32, subdivision: &str) -> Result<bool>;
}

// ---------------------------------------------------------------------------
// VoterRegistration — registro eleitoral opcional
// ---------------------------------------------------------------------------

/// Porta plugável do **registro eleitoral** do país (conceito opcional; âncora fraca). BR = Título
/// de Eleitor (12 dígitos + 2 DV, algoritmo TSE). A validação é pura/offline; a promoção a
/// `verified` (cross-check com a fonte oficial) fica a cargo da localização.
pub trait VoterRegistration: Send + Sync {
    /// Valida algoritmicamente o número do registro e devolve a forma **normalizada** (só dígitos).
    ///
    /// # Errors
    /// [`crate::Error::Validation`] com mensagem pública (pt-BR na instalação BR) quando inválido.
    fn validate(&self, raw: &str) -> Result<String>;
}

// ---------------------------------------------------------------------------
// Localization — o bundle resolvido da configuração da instalação
// ---------------------------------------------------------------------------

/// A localização ativa da instalação (Pindorama → `l10n_br`). Agrupa os provedores pra que o
/// wiring resolva um único `Arc<dyn Localization>` a partir do código do país configurado.
pub trait Localization: Send + Sync {
    /// Código ISO-3166-1 alfa-2 do país (ex.: `"BR"`).
    fn country_code(&self) -> &'static str;

    /// Verificação de registro eleitoral, se o país tiver o conceito.
    fn voter_registration(&self) -> Option<&dyn VoterRegistration>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assurance_is_ordered_and_stable() {
        assert!(IdentityAssurance::Unverified < IdentityAssurance::Verified);
        assert_eq!(IdentityAssurance::Validated.as_str(), "validated");
    }

    #[test]
    fn skipped_outcome_fails_open() {
        let s = IdentityOutcome::skipped();
        assert!(s.allows_registration());
        assert!(!s.is_strong());
        assert!(!s.needs_review());
    }

    #[test]
    fn outcome_decisions() {
        let accept = IdentityOutcome {
            found: true,
            band: IdentityBand::Accept,
        };
        assert!(accept.is_strong() && accept.allows_registration());
        let reject = IdentityOutcome {
            found: true,
            band: IdentityBand::Reject,
        };
        assert!(!reject.allows_registration());
        let review = IdentityOutcome {
            found: true,
            band: IdentityBand::Review,
        };
        assert!(review.needs_review() && !review.is_strong());
    }
}

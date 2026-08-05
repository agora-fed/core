//! Localization (l10n): the core's **country-agnostic** abstractions (ADR-0015, Odoo
//! `l10n_br` style). The AGORA core knows nothing about CPF, voter registry or IBGE — it knows
//! only the three traits in this module. Each country plugs an `l10n_<cc>` module (e.g.
//! `dsoc-l10n-br`) that implements them. Identifiers in English (ADR-0013); country-specific UI
//! copy lives in the localization module.
//!
//! - [`IdentityVerifier`]    — checks an **identity document** (CPF, SSN, DNI, …).
//! - [`TerritorialProvider`] — the **country → state → municipality** hierarchy (the scope axis
//!   for sortition/federation/campaigns).
//! - [`VoterRegistration`]   — the **optional** notion of an electoral registry.

use crate::error::Result;

// ---------------------------------------------------------------------------
// IdentityVerifier — identity document
// ---------------------------------------------------------------------------

/// Assurance level of an identity document — weakest to strongest.
///
/// Maps directly onto the former `CpfStatus` from `l10n_br`, in neutral terms: `Validated` is
/// the algorithmic check (check digits), `Verified` is confirmation against an official source
/// of the country.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IdentityAssurance {
    /// Not checked yet.
    Unverified,
    /// Algorithmically valid (check digits).
    Validated,
    /// Confirmed against the country's official source (KYC/registry).
    Verified,
}

impl IdentityAssurance {
    /// Stable form for persistence/audit (compatible with the current schema: `unverified` /
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

/// Confidence band of an identity match against the country's authorized base (not a
/// probability — a calibrated band). Neutral: each localization maps its service's own
/// vocabulary (e.g. `ACEITA`/`REVISA`/`ESCALA`/`REJEITA` in BR) onto these variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityBand {
    /// Clear match — strong identity.
    Accept,
    /// Matches, but warrants human review.
    Review,
    /// Ambiguous signal — escalate.
    Escalate,
    /// No match — block.
    Reject,
    /// Verification not performed (service absent/down) — graceful degradation.
    Skipped,
}

/// What is submitted to match a document against the authorized base (country-agnostic).
#[derive(Debug, Clone)]
pub struct IdentityCheck {
    /// Normalized document number (CPF, SSN, DNI, …), punctuation stripped.
    pub document_id: String,
    /// Full name as given by the person.
    pub full_name: String,
    /// Date of birth `YYYY-MM-DD`, when provided.
    pub birth_date: Option<String>,
    /// Sex (`M`/`F`), when provided.
    pub sex: Option<String>,
}

/// Verdict-only outcome of an identity match (never returns the stored data).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IdentityOutcome {
    /// Did the base find the document?
    pub found: bool,
    /// The calibrated band of the match.
    pub band: IdentityBand,
}

impl IdentityOutcome {
    /// The "not verified" verdict — used for graceful degradation (fail-open).
    #[must_use]
    pub fn skipped() -> Self {
        Self {
            found: false,
            band: IdentityBand::Skipped,
        }
    }

    /// May registration proceed? Everything but `Reject` does (including Skipped — fail-open).
    #[must_use]
    pub fn allows_registration(&self) -> bool {
        !matches!(self.band, IdentityBand::Reject)
    }

    /// Strongly confirmed identity (`Accept`) → candidate for `IdentityAssurance::Verified`.
    #[must_use]
    pub fn is_strong(&self) -> bool {
        matches!(self.band, IdentityBand::Accept)
    }

    /// Needs human review (`Review`/`Escalate`).
    #[must_use]
    pub fn needs_review(&self) -> bool {
        matches!(self.band, IdentityBand::Review | IdentityBand::Escalate)
    }
}

/// Pluggable port for verifying the country's **identity document**. The localization plugs in
/// the implementation (BR = CPF: check digits + the cpf-verify SaaS). Never panics; a transport
/// error becomes [`IdentityOutcome::skipped`].
#[async_trait::async_trait]
pub trait IdentityVerifier: Send + Sync {
    /// Match the query against the country's authorized base.
    async fn verify_identity(&self, check: &IdentityCheck) -> IdentityOutcome;
}

// ---------------------------------------------------------------------------
// TerritorialProvider — country → state → municipality
// ---------------------------------------------------------------------------

/// A municipality (the base territorial unit). Neutral: in BR the `code` is the IBGE code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Municipality {
    /// Stable code of the unit (IBGE in BR).
    pub code: i32,
    /// Display name.
    pub name: String,
}

/// Pluggable port for the country's territorial hierarchy. The localization plugs in the source
/// (BR = the `municipio_ibge` table). This is the scope axis for sortition/federation/campaigns
/// (ADR-0014).
#[async_trait::async_trait]
pub trait TerritorialProvider: Send + Sync {
    /// Municipalities of a first-level subdivision (UF in BR), ordered by name.
    ///
    /// # Errors
    /// [`crate::Error::Storage`] on a persistence failure.
    async fn municipalities(&self, subdivision: &str) -> Result<Vec<Municipality>>;

    /// Does municipality `code` belong to `subdivision`? (Residence validation.)
    ///
    /// # Errors
    /// [`crate::Error::Storage`] on a persistence failure.
    async fn municipality_in_subdivision(&self, code: i32, subdivision: &str) -> Result<bool>;
}

// ---------------------------------------------------------------------------
// VoterRegistration — registro eleitoral opcional
// ---------------------------------------------------------------------------

/// Pluggable port for the country's **electoral registry** (optional concept; weak anchor).
/// BR = the electoral registry (12 digits + 2 check digits, TSE algorithm). Validation is
/// pure/offline; promotion to `verified` (cross-check against the official source) is the
/// localization's responsibility.
pub trait VoterRegistration: Send + Sync {
    /// Algorithmically validate the registry number and return the **normalized** form (digits only).
    ///
    /// # Errors
    /// [`crate::Error::Validation`] with a public message (pt-BR on the BR installation) when invalid.
    fn validate(&self, raw: &str) -> Result<String>;
}

// ---------------------------------------------------------------------------
// Localization — the bundle resolved from the installation's configuration
// ---------------------------------------------------------------------------

/// The installation's active localization (Pindorama → `l10n_br`). Groups the providers so the
/// wiring resolves a single `Arc<dyn Localization>` from the configured country code.
pub trait Localization: Send + Sync {
    /// ISO-3166-1 alpha-2 country code (e.g. `"BR"`).
    fn country_code(&self) -> &'static str;

    /// Electoral-registry verification, when the country has the concept.
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

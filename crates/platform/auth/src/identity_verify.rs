//! Identity verification against the authorized document base (the `cpf-verify` SaaS), R-KYC #49.
//!
//! **ADR-0015:** this logic is Brazil-specific (it matches CPF + name + birth date + sex against
//! the Brazilian authorized base) and was moved behind the localization boundary in
//! [`dsoc_l10n_br::saas`]. We re-export the same items here to preserve the
//! `crate::identity_verify::*` paths used by the signup flow ([`crate::signup_verify`]). The
//! country-agnostic abstraction is [`dsoc_core::IdentityVerifier`], implemented for BR by
//! [`dsoc_l10n_br::BrIdentityVerifier`].

pub use dsoc_l10n_br::saas::{
    from_env, Faixa, HttpIdentityVerifier, IdentityQuery, IdentityVerdict, IdentityVerifier,
    NoopIdentityVerifier,
};

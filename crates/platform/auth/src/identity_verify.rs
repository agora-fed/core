//! Verificação de identidade contra a base autorizada de CPFs (SaaS `cpf-verify`), R-KYC #49.
//!
//! **ADR-0015:** esta lógica é Brasil-específica (confronta CPF + nome + nascimento + sexo com a
//! base autorizada brasileira) e foi movida para trás da fronteira de localização em
//! [`dsoc_l10n_br::saas`]. Reexportamos aqui os mesmos itens para preservar os caminhos
//! `crate::identity_verify::*` usados pelo fluxo de cadastro ([`crate::signup_verify`]). A
//! abstração agnóstica de país é [`dsoc_core::IdentityVerifier`], implementada no BR por
//! [`dsoc_l10n_br::BrIdentityVerifier`].

pub use dsoc_l10n_br::saas::{
    from_env, Faixa, HttpIdentityVerifier, IdentityQuery, IdentityVerdict, IdentityVerifier,
    NoopIdentityVerifier,
};

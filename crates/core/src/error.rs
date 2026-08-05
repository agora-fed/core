//! The canonical error model. Every crate maps its failures into [`Error`] so the
//! gateway can render consistent, non-leaking responses (coding-style: handle errors
//! explicitly; never leak sensitive data — SECURITY.md).

use thiserror::Error;

/// Workspace-wide result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Canonical domain error. Variants are intentionally coarse and stable: they are part
/// of the frozen contract and map onto HTTP status codes in the gateway.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// The requested entity does not exist.
    #[error("not found: {0}")]
    NotFound(String),

    /// The caller is authenticated but not allowed to perform this action.
    #[error("forbidden: {0}")]
    Forbidden(String),

    /// The caller is not authenticated.
    #[error("unauthorized")]
    Unauthorized,

    /// Input failed validation at a system boundary (coding-style: validate at boundaries).
    #[error("invalid input: {0}")]
    Validation(String),

    /// A domain rule was violated (e.g. voting on a closed proposal).
    #[error("conflict: {0}")]
    Conflict(String),

    /// A database / persistence failure. The detail is logged server-side, not returned raw.
    #[error("storage error")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// A failure talking to a sovereign dependency (Zitadel, embeddings, SMTP, …).
    #[error("dependency '{dependency}' failed")]
    Dependency {
        /// The dependency name (for logs/metrics), never sensitive.
        dependency: &'static str,
        /// The underlying cause (logged, not surfaced to the public API).
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The caller exceeded a rate limit (per IP, per citizen, and so on).
    /// The public-safe pt-BR message travels in the wrapper — the domain layer
    /// picks *how many* / *in which window* according to the policy at hand.
    #[error("rate limited: {0}")]
    RateLimit(String),
}

impl Error {
    /// Stable, public-safe error code for API envelopes (no internal detail leaks).
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Error::NotFound(_) => "not_found",
            Error::Forbidden(_) => "forbidden",
            Error::Unauthorized => "unauthorized",
            Error::Validation(_) => "invalid_input",
            Error::Conflict(_) => "conflict",
            Error::Storage(_) => "storage_error",
            Error::Dependency { .. } => "dependency_error",
            Error::RateLimit(_) => "rate_limited",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable_and_public_safe() {
        assert_eq!(Error::Unauthorized.code(), "unauthorized");
        assert_eq!(Error::NotFound("x".into()).code(), "not_found");
        // Storage errors must not surface internal detail in their Display.
        let inner = std::io::Error::other("secret connection string");
        let e = Error::Storage(Box::new(inner));
        assert_eq!(e.to_string(), "storage error");
    }
}

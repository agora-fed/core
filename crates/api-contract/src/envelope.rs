//! The consistent response envelope used by every endpoint (org pattern: a uniform
//! `{ success, data, error, meta }` shape so clients handle all responses identically).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// A public-safe API error (mirrors `dsoc_core::Error::code`; never leaks internals).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ApiError {
    /// Stable machine code, e.g. `"not_found"`, `"forbidden"`.
    pub code: String,
    /// Human-readable, non-sensitive message (Portuguese for end users).
    pub message: String,
}

/// Pagination metadata for list responses (PLAN.md: unbounded reads must paginate).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PageMeta {
    /// Total matching records (or a lower bound for keyset pagination).
    pub total: u64,
    /// Page size requested.
    pub limit: u32,
    /// Zero-based offset (or interpreted as a keyset cursor index).
    pub offset: u32,
}

/// The uniform envelope. Exactly one of `data`/`error` is populated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    /// Whether the call succeeded.
    pub success: bool,
    /// The payload on success.
    pub data: Option<T>,
    /// The error on failure.
    pub error: Option<ApiError>,
    /// Pagination metadata for list endpoints.
    pub meta: Option<PageMeta>,
}

impl<T> ApiResponse<T> {
    /// A successful single-item response.
    pub fn ok(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            meta: None,
        }
    }

    /// A successful paginated response.
    pub fn page(data: T, meta: PageMeta) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            meta: Some(meta),
        }
    }

    /// A failure response.
    pub fn fail(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(ApiError {
                code: code.into(),
                message: message.into(),
            }),
            meta: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_and_failure_are_mutually_exclusive() {
        let ok = ApiResponse::ok(42u32);
        assert!(ok.success && ok.data.is_some() && ok.error.is_none());

        let err = ApiResponse::<u32>::fail("forbidden", "Acesso negado");
        assert!(!err.success && err.data.is_none() && err.error.is_some());
    }
}

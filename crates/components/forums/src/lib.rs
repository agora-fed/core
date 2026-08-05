//! # dsoc-forums — hierarchical institutional forums
//!
//! The platform's thesis applied to forums (`/f/<path>`): society deliberates,
//! LOCAL interactions cross configurable thresholds, and each threshold fires an
//! e-mail to the responsible institution with a public receipt. Federated
//! (fediverse) interactions count SEPARATELY and never fire a dispatch.
//!
//! The structure mirrors the other components (proposals/debates): pure `domain`,
//! compile-time `queries` (sqlx), transactional `service`, axum `http`.

pub mod domain;
pub mod http;
pub mod queries;
pub mod service;

pub use domain::{territorial_sections, NewTopic, TerritorialSection, MAX_DEPTH};
pub use http::routes;
pub use service::ForumService;

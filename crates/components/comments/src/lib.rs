//! # dsoc-comments
//!
//! Tier 2 crate. Threaded deliberation on any votable entity.
//!
//! ## Contract
//! - **Emits:** comments.created
//! - **Consumes:** moderation.flagged
//! - **Owns tables:** comment, comment_vote
//!
//! This crate talks to the rest of the system ONLY through `dsoc-core` traits,
//! the event bus (`dsoc-events`), and the gateway. It never reaches into another
//! crate's internals (see `DO NOT` in PLAN.md and CONTRIBUTING.md).

#![forbid(unsafe_code)]

pub mod domain;
pub mod events;
pub mod http;
pub mod queries;
pub mod service;

pub use events::{comment_created_envelope, handle_event, moderation_flagged};
pub use http::routes;
pub use service::CommentService;

/// Compile-time marker proving the crate name is wired into the workspace.
pub const CRATE_NAME: &str = "dsoc-comments";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_stable() {
        // Guards against accidental rename that would break event routing.
        assert_eq!(CRATE_NAME, "dsoc-comments");
    }
}

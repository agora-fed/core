//! The comments service: orchestrates the pure domain, the `sqlx` queries, and the
//! injected [`Clock`]. It maps storage failures onto the canonical [`dsoc_core::Error`].
//!
//! Two invariants are enforced here transactionally:
//! 1. **Atomic emission (ADR-0006).** A new comment and its `comments.created` event are
//!    written in the *same* transaction via the outbox, so a committed comment can never
//!    lose its event and a published event can never reference an uncommitted comment.
//! 2. **Race-free placement.** A reply locks its parent `FOR UPDATE`, validates it belongs
//!    to the same proposal, and applies the pure depth guard — closing the TOCTOU window
//!    between "parent exists" and "child inserted".
//!
//! Time is read only from the injected [`Clock`] (never ambient — TESTING.md).

use std::sync::Arc;

use dsoc_core::clock::Clock;
use dsoc_core::error::{Error, Result};
use dsoc_core::ids::{CitizenId, CommentId, OrgId, ProposalId};
use dsoc_db::Db;
use uuid::Uuid;

use crate::domain::{self, Comment, CommentStatus, CommentVote, VoteWeight};
use crate::events;
use crate::queries::{self, Cursor};

/// Default page size for the thread listing.
pub const DEFAULT_PAGE_LIMIT: i64 = 50;
/// Hard cap on page size to keep reads bounded.
pub const MAX_PAGE_LIMIT: i64 = 200;

/// Threaded deliberation service. Cheap to construct per request from `AppState`.
pub struct CommentService {
    db: Db,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for CommentService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommentService").finish_non_exhaustive()
    }
}

impl CommentService {
    /// Build a service from its injected ports. Events are emitted through the
    /// transactional outbox (ADR-0006), so no [`dsoc_core::EventBus`] is injected here.
    #[must_use]
    pub fn new(db: Db, clock: Arc<dyn Clock>) -> Self {
        Self { db, clock }
    }

    /// Build a service from the shared application state.
    #[must_use]
    pub fn from_state(state: &dsoc_app::AppState) -> Self {
        Self::new(state.db.clone(), state.clock.clone())
    }

    /// Create a comment — a root when `parent` is `None`, otherwise a reply. The new
    /// comment and its `comments.created` event commit atomically through the outbox.
    ///
    /// A reply locks its parent `FOR UPDATE`, requires the parent to exist (a missing
    /// parent is a clean [`Error::Validation`], never a raw FK error) and to belong to the
    /// same proposal, then derives the child depth via the pure guard — rejecting a thread
    /// deeper than [`domain::MAX_THREAD_DEPTH`] as [`Error::Conflict`].
    ///
    /// # Errors
    /// - [`Error::Validation`] for an empty/oversized body, a missing parent, or a parent
    ///   on a different proposal.
    /// - [`Error::Conflict`] when the reply would exceed the maximum thread depth.
    /// - [`Error::Storage`] on any persistence/publish failure.
    pub async fn create_comment(
        &self,
        org: OrgId,
        proposal: ProposalId,
        parent: Option<CommentId>,
        author: CitizenId,
        body: &str,
    ) -> Result<Comment> {
        let body = domain::validate_body(body).map_err(validation)?;
        let now = self.clock.now();

        let mut tx = self.db.begin().await.map_err(map_sqlx)?;

        let (parent_id, depth) = match parent {
            None => (None, 0),
            Some(parent_id) => {
                let parent_ref = queries::lock_parent(&mut *tx, parent_id.as_uuid())
                    .await
                    .map_err(map_storage)?
                    .ok_or_else(|| Error::Validation("parent comment does not exist".to_owned()))?;
                // A reply may not jump to a different proposal's thread.
                if parent_ref.proposal_id != proposal.as_uuid() {
                    return Err(Error::Validation(
                        "parent comment belongs to a different proposal".to_owned(),
                    ));
                }
                let depth = domain::child_depth(Some(parent_ref.depth))
                    .map_err(|e| Error::Conflict(e.to_string()))?;
                (Some(parent_id.as_uuid()), depth)
            }
        };

        let comment = Comment {
            id: CommentId::new().as_uuid(),
            org_id: org.as_uuid(),
            proposal_id: proposal.as_uuid(),
            parent_id,
            author_id: author.as_uuid(),
            body,
            depth,
            status: CommentStatus::Visible,
            created_at: now,
        };

        queries::insert_comment(&mut *tx, &comment)
            .await
            .map_err(map_storage)?;

        // Emit inside the same transaction as the write (ADR-0006).
        let envelope = events::comment_created_envelope(
            &self.clock,
            org,
            CommentId::from_uuid(comment.id),
            proposal,
        );
        dsoc_db::outbox::publish_tx(&mut *tx, &envelope)
            .await
            .map_err(map_sqlx)?;

        tx.commit().await.map_err(map_sqlx)?;
        Ok(comment)
    }

    /// Fetch a single comment.
    ///
    /// # Errors
    /// [`Error::NotFound`] when absent; [`Error::Storage`] otherwise.
    pub async fn get_comment(&self, id: CommentId) -> Result<Comment> {
        queries::get_comment(&self.db, id.as_uuid())
            .await
            .map_err(map_storage)
    }

    /// List a proposal's thread oldest-first (keyset-paginated).
    ///
    /// # Errors
    /// [`Error::Storage`] on a storage failure.
    pub async fn list_thread(
        &self,
        org: OrgId,
        proposal: ProposalId,
        cursor: Option<Cursor>,
        limit: i64,
    ) -> Result<Vec<Comment>> {
        queries::list_thread(
            &self.db,
            org.as_uuid(),
            proposal.as_uuid(),
            cursor,
            clamp_limit(limit),
        )
        .await
        .map_err(map_storage)
    }

    /// Cast (or change) a citizen's up/down vote on a comment. Idempotent per citizen: a
    /// repeat vote updates the weight in place and returns the REAL stored row (its
    /// original id and creation time), never a duplicate or a phantom id.
    ///
    /// # Errors
    /// [`Error::Validation`] when the comment or citizen is unknown (mapped from the FK
    /// violation); [`Error::Storage`] otherwise.
    pub async fn vote(
        &self,
        comment: CommentId,
        citizen: CitizenId,
        weight: VoteWeight,
    ) -> Result<CommentVote> {
        queries::upsert_vote(
            &self.db,
            Uuid::now_v7(),
            comment.as_uuid(),
            citizen.as_uuid(),
            weight.as_i16(),
            self.clock.now(),
        )
        .await
        .map_err(map_storage)
    }

    /// The net score (sum of `{-1, +1}` weights) of a comment — an aggregate that never
    /// exposes per-citizen linkage.
    ///
    /// # Errors
    /// [`Error::Storage`] on a storage failure.
    pub async fn score(&self, comment: CommentId) -> Result<i64> {
        queries::comment_score(&self.db, comment.as_uuid())
            .await
            .map_err(map_storage)
    }

    /// Flag every still-visible comment of a proposal (the `moderation.flagged` fan-out).
    /// Returns the number transitioned. Idempotent under at-least-once delivery: the UPDATE
    /// is guarded by the `visible` prior state, so a redundant call flags zero rows.
    ///
    /// # Errors
    /// [`Error::Storage`] on a storage failure.
    pub async fn flag_proposal_comments(&self, org: OrgId, proposal: ProposalId) -> Result<u64> {
        queries::flag_visible_comments(&self.db, org.as_uuid(), proposal.as_uuid())
            .await
            .map_err(map_storage)
    }
}

fn clamp_limit(limit: i64) -> i64 {
    limit.clamp(1, MAX_PAGE_LIMIT)
}

fn validation(err: domain::ParseError) -> Error {
    Error::Validation(err.to_string())
}

/// Map a storage-layer [`queries::Error`] onto the canonical [`dsoc_core::Error`]
/// (sqlx `RowNotFound` -> `NotFound`; unique violation -> `Conflict`; FK/check/not-null
/// violation -> `Validation`; everything else -> `Storage`).
fn map_storage(err: queries::Error) -> Error {
    match err {
        queries::Error::Db(sqlx::Error::RowNotFound) => {
            Error::NotFound("comment not found".to_owned())
        }
        queries::Error::Db(sqlx::Error::Database(db_err)) => match db_err.code().as_deref() {
            // unique_violation
            Some("23505") => Error::Conflict("comment vote already exists".to_owned()),
            // foreign_key / check / not_null violation -> bad input
            Some("23503" | "23514" | "23502") => {
                Error::Validation("comment input violates a constraint".to_owned())
            }
            _ => Error::Storage(Box::new(db_err)),
        },
        queries::Error::Db(other) => Error::Storage(Box::new(other)),
        // A corrupt stored value is an internal integrity failure, not a client error.
        queries::Error::Decode(parse) => Error::Storage(Box::new(parse)),
    }
}

/// Map a raw `sqlx` failure (from tx begin/commit or the outbox insert) onto the canonical
/// error model.
fn map_sqlx(err: sqlx::Error) -> Error {
    match err {
        sqlx::Error::RowNotFound => Error::NotFound("comment not found".to_owned()),
        sqlx::Error::Database(ref db) if db.is_unique_violation() => {
            Error::Conflict("comment already exists".to_owned())
        }
        other => Error::Storage(Box::new(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_limit_bounds_page_size() {
        assert_eq!(clamp_limit(0), 1);
        assert_eq!(clamp_limit(-10), 1);
        assert_eq!(clamp_limit(50), 50);
        assert_eq!(clamp_limit(10_000), MAX_PAGE_LIMIT);
    }

    #[test]
    fn row_not_found_maps_to_not_found() {
        let mapped = map_storage(queries::Error::Db(sqlx::Error::RowNotFound));
        assert_eq!(mapped.code(), "not_found");
    }

    #[test]
    fn decode_error_maps_to_storage() {
        let parse = domain::ParseError {
            field: "status",
            value: "garbage".to_owned(),
        };
        let mapped = map_storage(queries::Error::Decode(parse));
        assert_eq!(mapped.code(), "storage_error");
    }

    #[test]
    fn validation_helper_maps_parse_error() {
        let parse = domain::ParseError {
            field: "body",
            value: String::new(),
        };
        assert!(matches!(validation(parse), Error::Validation(_)));
    }

    #[test]
    fn map_sqlx_other_is_storage() {
        assert_eq!(map_sqlx(sqlx::Error::PoolTimedOut).code(), "storage_error");
    }
}

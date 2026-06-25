//! Every database statement for moderation, as `sqlx` compile-time-checked queries
//! (PLAN.md principle 3 — no ORM, no `SELECT *`, keyset pagination on unbounded reads).
//!
//! Functions return domain types from [`crate::domain`]. Text-coded enums are decoded
//! here; a decode failure means corrupt storage and surfaces as [`Error::Decode`]. The
//! service layer maps [`Error`] onto the canonical [`dsoc_core::Error`].

use chrono::{DateTime, Utc};
use dsoc_db::Db;
use uuid::Uuid;

use crate::domain::{
    Appeal, AppealStatus, Decision, Outcome, ParseError, Rule, RuleAction, RuleKind, TargetKind,
};

/// A storage-layer failure: a `sqlx` error or a value that does not decode into the
/// domain (a corrupt row, which the `CHECK` constraints should prevent).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The underlying database call failed.
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    /// A stored value could not be decoded into the domain model.
    #[error(transparent)]
    Decode(#[from] ParseError),
}

/// Keyset cursor for newest-first pagination: rows strictly older than `(at, id)`.
#[derive(Debug, Clone, Copy)]
pub struct Cursor {
    /// The `created_at` of the last row from the previous page.
    pub at: DateTime<Utc>,
    /// The `id` of the last row from the previous page (tie-breaker).
    pub id: Uuid,
}

// --- row shapes (kept private; decoded into domain via the helpers below) -----------

struct RuleRow {
    id: Uuid,
    org_id: Uuid,
    kind: String,
    pattern: String,
    action: String,
    created_at: DateTime<Utc>,
}

impl RuleRow {
    fn into_domain(self) -> Result<Rule, ParseError> {
        Ok(Rule {
            id: self.id,
            org_id: self.org_id,
            kind: self.kind.parse::<RuleKind>()?,
            pattern: self.pattern,
            action: self.action.parse::<RuleAction>()?,
            created_at: self.created_at,
        })
    }
}

struct DecisionRow {
    id: Uuid,
    org_id: Uuid,
    target_kind: String,
    target_id: Uuid,
    rule_id: Option<Uuid>,
    outcome: String,
    created_at: DateTime<Utc>,
}

impl DecisionRow {
    fn into_domain(self) -> Result<Decision, ParseError> {
        Ok(Decision {
            id: self.id,
            org_id: self.org_id,
            target_kind: self.target_kind.parse::<TargetKind>()?,
            target_id: self.target_id,
            rule_id: self.rule_id,
            outcome: self.outcome.parse::<Outcome>()?,
            created_at: self.created_at,
        })
    }
}

struct AppealRow {
    id: Uuid,
    decision_id: Uuid,
    reason: String,
    status: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl AppealRow {
    fn into_domain(self) -> Result<Appeal, ParseError> {
        Ok(Appeal {
            id: self.id,
            decision_id: self.decision_id,
            reason: self.reason,
            status: self.status.parse::<AppealStatus>()?,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

// --- rules --------------------------------------------------------------------------

/// Persist a new rule. The caller supplies the id and timestamp (from the clock).
///
/// # Errors
/// [`Error::Db`] on a storage failure (e.g. unknown `org_id` or duplicate id).
pub async fn insert_rule(db: &Db, rule: &Rule) -> Result<(), Error> {
    sqlx::query!(
        r#"
        INSERT INTO moderation_rule (id, org_id, kind, pattern, action, created_at)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
        rule.id,
        rule.org_id,
        rule.kind.as_str(),
        rule.pattern,
        rule.action.as_str(),
        rule.created_at,
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Fetch an organization's rules oldest-first for deterministic evaluation, capped by
/// `limit` (bounded read — moderation rulesets are small and human-authored).
///
/// # Errors
/// [`Error::Db`] on a storage failure; [`Error::Decode`] on a corrupt row.
pub async fn fetch_rules_for_eval(db: &Db, org_id: Uuid, limit: i64) -> Result<Vec<Rule>, Error> {
    let rows = sqlx::query_as!(
        RuleRow,
        r#"
        SELECT id, org_id, kind, pattern, action, created_at
        FROM moderation_rule
        WHERE org_id = $1
        ORDER BY created_at ASC, id ASC
        LIMIT $2
        "#,
        org_id,
        limit,
    )
    .fetch_all(db)
    .await?;
    rows.into_iter()
        .map(RuleRow::into_domain)
        .collect::<Result<_, _>>()
        .map_err(Error::from)
}

/// List an organization's rules newest-first with keyset pagination.
///
/// # Errors
/// [`Error::Db`] on a storage failure; [`Error::Decode`] on a corrupt row.
pub async fn list_rules(
    db: &Db,
    org_id: Uuid,
    cursor: Option<Cursor>,
    limit: i64,
) -> Result<Vec<Rule>, Error> {
    let (cursor_at, cursor_id) = split_cursor(cursor);
    let rows = sqlx::query_as!(
        RuleRow,
        r#"
        SELECT id, org_id, kind, pattern, action, created_at
        FROM moderation_rule
        WHERE org_id = $1
          AND ($2::timestamptz IS NULL OR (created_at, id) < ($2, $3::uuid))
        ORDER BY created_at DESC, id DESC
        LIMIT $4
        "#,
        org_id,
        cursor_at,
        cursor_id,
        limit,
    )
    .fetch_all(db)
    .await?;
    rows.into_iter()
        .map(RuleRow::into_domain)
        .collect::<Result<_, _>>()
        .map_err(Error::from)
}

// --- decisions ----------------------------------------------------------------------

/// Persist a moderation decision (the audit record). Called for every evaluation.
///
/// # Errors
/// [`Error::Db`] on a storage failure.
pub async fn insert_decision(db: &Db, decision: &Decision) -> Result<(), Error> {
    sqlx::query!(
        r#"
        INSERT INTO moderation_decision
            (id, org_id, target_kind, target_id, rule_id, outcome, created_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
        decision.id,
        decision.org_id,
        decision.target_kind.as_str(),
        decision.target_id,
        decision.rule_id,
        decision.outcome.as_str(),
        decision.created_at,
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Fetch a single decision by id.
///
/// # Errors
/// [`sqlx::Error::RowNotFound`] (wrapped in [`Error::Db`]) when absent; [`Error::Decode`]
/// on a corrupt row.
pub async fn get_decision(db: &Db, id: Uuid) -> Result<Decision, Error> {
    let row = sqlx::query_as!(
        DecisionRow,
        r#"
        SELECT id, org_id, target_kind, target_id, rule_id, outcome, created_at
        FROM moderation_decision
        WHERE id = $1
        "#,
        id,
    )
    .fetch_one(db)
    .await?;
    Ok(row.into_domain()?)
}

/// List an organization's decisions newest-first with keyset pagination (the audit log).
///
/// # Errors
/// [`Error::Db`] on a storage failure; [`Error::Decode`] on a corrupt row.
pub async fn list_decisions(
    db: &Db,
    org_id: Uuid,
    cursor: Option<Cursor>,
    limit: i64,
) -> Result<Vec<Decision>, Error> {
    let (cursor_at, cursor_id) = split_cursor(cursor);
    let rows = sqlx::query_as!(
        DecisionRow,
        r#"
        SELECT id, org_id, target_kind, target_id, rule_id, outcome, created_at
        FROM moderation_decision
        WHERE org_id = $1
          AND ($2::timestamptz IS NULL OR (created_at, id) < ($2, $3::uuid))
        ORDER BY created_at DESC, id DESC
        LIMIT $4
        "#,
        org_id,
        cursor_at,
        cursor_id,
        limit,
    )
    .fetch_all(db)
    .await?;
    rows.into_iter()
        .map(DecisionRow::into_domain)
        .collect::<Result<_, _>>()
        .map_err(Error::from)
}

// --- appeals ------------------------------------------------------------------------

/// Persist a new appeal in the `open` state.
///
/// # Errors
/// [`Error::Db`] on a storage failure (e.g. unknown `decision_id`).
pub async fn insert_appeal(db: &Db, appeal: &Appeal) -> Result<(), Error> {
    sqlx::query!(
        r#"
        INSERT INTO moderation_appeal
            (id, decision_id, reason, status, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
        appeal.id,
        appeal.decision_id,
        appeal.reason,
        appeal.status.as_str(),
        appeal.created_at,
        appeal.updated_at,
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Fetch a single appeal by id.
///
/// # Errors
/// [`sqlx::Error::RowNotFound`] (wrapped in [`Error::Db`]) when absent; [`Error::Decode`]
/// on a corrupt row.
pub async fn get_appeal(db: &Db, id: Uuid) -> Result<Appeal, Error> {
    let row = sqlx::query_as!(
        AppealRow,
        r#"
        SELECT id, decision_id, reason, status, created_at, updated_at
        FROM moderation_appeal
        WHERE id = $1
        "#,
        id,
    )
    .fetch_one(db)
    .await?;
    Ok(row.into_domain()?)
}

/// Apply a resolved status to an appeal and return the updated row. The state-machine
/// guard lives in the domain; this writes the already-validated transition.
///
/// # Errors
/// [`sqlx::Error::RowNotFound`] (wrapped in [`Error::Db`]) when absent; [`Error::Decode`]
/// on a corrupt row.
pub async fn update_appeal_status(
    db: &Db,
    id: Uuid,
    status: AppealStatus,
    updated_at: DateTime<Utc>,
) -> Result<Appeal, Error> {
    let row = sqlx::query_as!(
        AppealRow,
        r#"
        UPDATE moderation_appeal
        SET status = $2, updated_at = $3
        WHERE id = $1
        RETURNING id, decision_id, reason, status, created_at, updated_at
        "#,
        id,
        status.as_str(),
        updated_at,
    )
    .fetch_one(db)
    .await?;
    Ok(row.into_domain()?)
}

fn split_cursor(cursor: Option<Cursor>) -> (Option<DateTime<Utc>>, Option<Uuid>) {
    match cursor {
        Some(c) => (Some(c.at), Some(c.id)),
        None => (None, None),
    }
}

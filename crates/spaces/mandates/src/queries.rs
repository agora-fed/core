//! Every `sqlx` statement the crate runs, in one place and compile-time checked (PLAN.md
//! principle 3 — explicit, auditable SQL; no ORM, no `SELECT *`, keyset pagination for lists).
//! Functions return raw `sqlx::Error`; [`crate::service`] maps it onto the canonical
//! [`dsoc_core::Error`] model.
//!
//! SECURITY: the invite token is a credential. It is hashed **inside PostgreSQL** with
//! `pgcrypto`'s `digest(... ,'sha256')` so the plaintext is never written to a column — both on
//! insert ([`insert_invitation`]) and on lookup ([`lock_invitation_by_token`]).

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

/// A mandate row read for invite / display (core-owned `mandate` table; the `mandates` crate owns
/// its lifecycle). No `SELECT *` — only the columns the lifecycle needs. The "real parliament"
/// columns (party/uf/house/avatar_object_key, added in migration 0201) are optional so the
/// historical seed rows keep loading without backfill.
#[derive(Debug, Clone)]
pub(crate) struct MandateRow {
    pub id: Uuid,
    pub office: String,
    pub display_name: String,
    pub public_email: String,
    pub is_candidate: bool,
    pub onboarded_at: Option<DateTime<Utc>>,
    pub party: Option<String>,
    pub uf: Option<String>,
    pub house: Option<String>,
    pub avatar_object_key: Option<String>,
    pub sphere: String,
}

/// The invitation + its mandate's onboarding marker, read under a row lock during acceptance.
#[derive(Debug, Clone)]
pub(crate) struct InvitationLockRow {
    pub invitation_id: Uuid,
    pub mandate_id: Uuid,
    pub accepted_at: Option<DateTime<Utc>>,
    pub sent_at: DateTime<Utc>,
    pub onboarded_at: Option<DateTime<Utc>>,
}

/// A persisted invitation row (its real stored id + send time, returned from the insert).
#[derive(Debug, Clone)]
pub(crate) struct InsertedInvitation {
    pub id: Uuid,
    pub sent_at: DateTime<Utc>,
}

/// A term-bound office record.
#[derive(Debug, Clone)]
pub(crate) struct OfficeRow {
    pub id: Uuid,
    pub mandate_id: Uuid,
    pub office: String,
    pub district: Option<String>,
    pub term_start: Option<NaiveDate>,
    pub term_end: Option<NaiveDate>,
    pub created_at: DateTime<Utc>,
}

/// An identity-binding row. `created_at` is written on insert but not surfaced (the audit trail
/// is keyed on `verified_at`), so it is intentionally not selected back.
#[derive(Debug, Clone)]
pub(crate) struct IdentityBindingRow {
    pub id: Uuid,
    pub mandate_id: Uuid,
    pub verification_level: String,
    pub evidence_ref: Option<String>,
    pub verified_at: DateTime<Utc>,
}

/// Reverse lookup: given a citizen, find the mandate they operate (if any). Used by `GET /me/
/// mandate` so the front knows whether to render the parliamentarian dashboard. Returns the
/// latest binding row's `verification_level` alongside the mandate row.
pub(crate) async fn find_mandate_by_operator<'e, E: PgExecutor<'e>>(
    exec: E,
    org_id: Uuid,
    citizen_id: Uuid,
) -> Result<Option<(MandateRow, String)>, sqlx::Error> {
    let row = sqlx::query!(
        r#"
        SELECT m.id, m.office, m.display_name, m.public_email, m.is_candidate, m.onboarded_at,
               m.party, m.uf, m.house, m.avatar_object_key, m.sphere,
               b.verification_level
          FROM mandate_identity_binding b
          JOIN mandate m ON m.id = b.mandate_id
         WHERE b.citizen_id = $1 AND m.org_id = $2
         ORDER BY b.verified_at DESC
         LIMIT 1
        "#,
        citizen_id,
        org_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.map(|r| {
        (
            MandateRow {
                id: r.id,
                office: r.office,
                display_name: r.display_name,
                public_email: r.public_email,
                is_candidate: r.is_candidate,
                onboarded_at: r.onboarded_at,
                party: r.party,
                uf: r.uf,
                house: r.house,
                avatar_object_key: r.avatar_object_key,
                sphere: r.sphere,
            },
            r.verification_level,
        )
    }))
}

/// Find a mandate within an organization (read for invite / display).
pub(crate) async fn find_mandate<'e, E: PgExecutor<'e>>(
    exec: E,
    org_id: Uuid,
    mandate_id: Uuid,
) -> Result<Option<MandateRow>, sqlx::Error> {
    let row = sqlx::query_as!(
        MandateRow,
        r#"
        SELECT id, office, display_name, public_email, is_candidate, onboarded_at,
               party, uf, house, avatar_object_key, sphere
        FROM mandate
        WHERE org_id = $1 AND id = $2
        "#,
        org_id,
        mandate_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

/// List mandates in an organization, ordered alphabetically by `display_name`. Returns up to
/// `limit` rows starting at `offset`. Used by the front-end picker on the "propose" page so people
/// don't have to type a UUID by hand — there is no compelling threat model for a hidden mandate, so
/// the read is public.
///
/// `uf`/`municipio` narrow the list to a single territory (case-insensitive via `upper()` on both
/// sides — the same comparison the `civic_source` catalog uses). Both are optional; absent means no
/// filter (backwards-compatible with the pre-municipal callers). Drives the "Vereadores desta
/// council-members card on a municipal forum.
pub(crate) async fn list_mandates<'e, E: PgExecutor<'e>>(
    exec: E,
    org_id: Uuid,
    sphere: Option<&str>,
    uf: Option<&str>,
    municipio: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<MandateRow>, sqlx::Error> {
    let rows = sqlx::query_as!(
        MandateRow,
        r#"
        SELECT id, office, display_name, public_email, is_candidate, onboarded_at,
               party, uf, house, avatar_object_key, sphere
        FROM mandate
        WHERE org_id = $1
          AND hidden_at IS NULL
          AND ($2::text IS NULL OR sphere = $2)
          AND ($3::text IS NULL OR upper(uf) = upper($3))
          AND ($4::text IS NULL OR upper(municipio) = upper($4))
        ORDER BY display_name ASC
        LIMIT $5 OFFSET $6
        "#,
        org_id,
        sphere,
        uf,
        municipio,
        limit,
        offset,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

/// Whether a mandate has at least one invitation row (drives the derived onboarding status).
pub(crate) async fn mandate_has_invitation<'e, E: PgExecutor<'e>>(
    exec: E,
    mandate_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let exists = sqlx::query_scalar!(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM mandate_invitation WHERE mandate_id = $1
        ) AS "exists!"
        "#,
        mandate_id,
    )
    .fetch_one(exec)
    .await?;
    Ok(exists)
}

/// Whether a mandate has a verified operator citizen bound (an entry in `mandate_identity_binding`
/// with a non-null `citizen_id` — the same semantics `find_mandate_by_operator` uses to populate
/// `MyMandateDto.binding_level`). Drives the public "vínculo verificado" badge on the mandate
/// profile without leaking the operator's identity — only the boolean.
///
/// Runtime `sqlx::query_scalar` (not the macro) so the committed `.sqlx/` offline cache does not
/// need regenerating on a DB-less build host (mirrors `parlamentar_activity::load_mandate_source`).
pub(crate) async fn mandate_has_verified_operator<'e, E: PgExecutor<'e>>(
    exec: E,
    mandate_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM mandate_identity_binding \
         WHERE mandate_id = $1 AND citizen_id IS NOT NULL)",
    )
    .bind(mandate_id)
    .fetch_one(exec)
    .await?;
    Ok(exists)
}

/// Persist a fresh invitation. The plaintext `token` is hashed in-database with SHA-256; only the
/// hex digest is stored in `token_hash`. Returns the REAL stored id and send time (RETURNING) —
/// never a pre-generated/phantom id.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn insert_invitation<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    mandate_id: Uuid,
    public_email: &str,
    token: &str,
    sent_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
) -> Result<InsertedInvitation, sqlx::Error> {
    let row = sqlx::query_as!(
        InsertedInvitation,
        r#"
        INSERT INTO mandate_invitation
            (id, mandate_id, public_email, token_hash, sent_at, accepted_at, created_at)
        VALUES ($1, $2, $3, encode(digest($4::text, 'sha256'), 'hex'), $5, NULL, $6)
        RETURNING id, sent_at
        "#,
        id,
        mandate_id,
        public_email,
        token,
        sent_at,
        created_at,
    )
    .fetch_one(exec)
    .await?;
    Ok(row)
}

/// Look up a pending invitation by the presented plaintext token and lock both the invitation and
/// its mandate row `FOR UPDATE` for the rest of the transaction (closes the onboarding TOCTOU).
/// The token is matched by its SHA-256 hash, computed in-database; the plaintext never lands in a
/// column. Returns `None` for an unknown token.
pub(crate) async fn lock_invitation_by_token<'e, E: PgExecutor<'e>>(
    exec: E,
    token: &str,
    org_id: Uuid,
) -> Result<Option<InvitationLockRow>, sqlx::Error> {
    let row = sqlx::query_as!(
        InvitationLockRow,
        r#"
        SELECT
            i.id           AS invitation_id,
            i.mandate_id   AS mandate_id,
            i.accepted_at  AS accepted_at,
            i.sent_at      AS sent_at,
            m.onboarded_at AS onboarded_at
        FROM mandate_invitation i
        JOIN mandate m ON m.id = i.mandate_id
        WHERE i.token_hash = encode(digest($1::text, 'sha256'), 'hex')
          AND m.org_id = $2
        FOR UPDATE OF i, m
        "#,
        token,
        org_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

/// Mark an invitation accepted, guarded by `accepted_at IS NULL` (optimistic concurrency).
/// Returns rows affected: 0 means a concurrent acceptance already claimed it.
pub(crate) async fn mark_invitation_accepted<'e, E: PgExecutor<'e>>(
    exec: E,
    invitation_id: Uuid,
    accepted_at: DateTime<Utc>,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query!(
        r#"
        UPDATE mandate_invitation
        SET accepted_at = $2
        WHERE id = $1 AND accepted_at IS NULL
        "#,
        invitation_id,
        accepted_at,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected())
}

/// Transition a mandate to onboarded, guarded by `onboarded_at IS NULL` (the expected prior
/// state). Returns rows affected: 0 means it was already onboarded (the already-in-target case).
pub(crate) async fn mark_mandate_onboarded<'e, E: PgExecutor<'e>>(
    exec: E,
    mandate_id: Uuid,
    onboarded_at: DateTime<Utc>,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query!(
        r#"
        UPDATE mandate
        SET onboarded_at = $2
        WHERE id = $1 AND onboarded_at IS NULL
        "#,
        mandate_id,
        onboarded_at,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected())
}

/// Append an immutable identity-assurance binding for a mandate. Returns the stored row.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn insert_identity_binding<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    mandate_id: Uuid,
    verification_level: &str,
    evidence_ref: Option<&str>,
    verified_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
) -> Result<IdentityBindingRow, sqlx::Error> {
    let row = sqlx::query_as!(
        IdentityBindingRow,
        r#"
        INSERT INTO mandate_identity_binding
            (id, mandate_id, verification_level, evidence_ref, verified_at, created_at)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id, mandate_id, verification_level, evidence_ref, verified_at
        "#,
        id,
        mandate_id,
        verification_level,
        evidence_ref,
        verified_at,
        created_at,
    )
    .fetch_one(exec)
    .await?;
    Ok(row)
}

/// Persist a term-bound office record. Returns the REAL stored row (RETURNING).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn insert_office<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    mandate_id: Uuid,
    office: &str,
    district: Option<&str>,
    term_start: Option<NaiveDate>,
    term_end: Option<NaiveDate>,
    created_at: DateTime<Utc>,
) -> Result<OfficeRow, sqlx::Error> {
    let row = sqlx::query_as!(
        OfficeRow,
        r#"
        INSERT INTO mandate_office
            (id, mandate_id, office, district, term_start, term_end, created_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id, mandate_id, office, district, term_start, term_end, created_at
        "#,
        id,
        mandate_id,
        office,
        district,
        term_start,
        term_end,
        created_at,
    )
    .fetch_one(exec)
    .await?;
    Ok(row)
}

/// Keyset-paginated office list for a mandate, ordered by id ascending. The cursor is the last id
/// of the previous page; pass `None` for the first page.
pub(crate) async fn list_offices<'e, E: PgExecutor<'e>>(
    exec: E,
    mandate_id: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<OfficeRow>, sqlx::Error> {
    let rows = sqlx::query_as!(
        OfficeRow,
        r#"
        SELECT id, mandate_id, office, district, term_start, term_end, created_at
        FROM mandate_office
        WHERE mandate_id = $1
          AND ($2::uuid IS NULL OR id > $2)
        ORDER BY id ASC
        LIMIT $3
        "#,
        mandate_id,
        after,
        limit,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

/// Keyset-paginated identity-binding history for a mandate, newest first. The cursor is the
/// `(verified_at, id)` of the last row of the previous page; pass `None` for the first page.
pub(crate) async fn list_identity_bindings<'e, E: PgExecutor<'e>>(
    exec: E,
    mandate_id: Uuid,
    cursor: Option<(DateTime<Utc>, Uuid)>,
    limit: i64,
) -> Result<Vec<IdentityBindingRow>, sqlx::Error> {
    let (cursor_at, cursor_id) = match cursor {
        Some((at, id)) => (Some(at), Some(id)),
        None => (None, None),
    };
    let rows = sqlx::query_as!(
        IdentityBindingRow,
        r#"
        SELECT id, mandate_id, verification_level, evidence_ref, verified_at
        FROM mandate_identity_binding
        WHERE mandate_id = $1
          AND ($2::timestamptz IS NULL OR (verified_at, id) < ($2, $3))
        ORDER BY verified_at DESC, id DESC
        LIMIT $4
        "#,
        mandate_id,
        cursor_at,
        cursor_id,
        limit,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

/// Whether an organization exists (the [`dsoc_core::traits::Space::ensure_open`] check — a mandate
/// registry space is hosted per organization).
pub(crate) async fn org_exists<'e, E: PgExecutor<'e>>(
    exec: E,
    org_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM org WHERE id = $1) AS "exists!""#,
        org_id,
    )
    .fetch_one(exec)
    .await?;
    Ok(exists)
}

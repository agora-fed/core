//! The administration service: orchestrates authorization, the injected clock, and the
//! `sqlx` queries, mapping every failure into the canonical [`dsoc_core::Error`].
//!
//! It holds the `Db` pool, an `Arc<dyn Clock>` (time is injected, never ambient), an
//! `Arc<dyn EventBus>` (the publish port — admin currently emits no cross-crate events,
//! see [`crate::events`]), and an `Arc<dyn Authorization>` used to gate every mutation.

use std::sync::Arc;

use dsoc_app::AppState;
use dsoc_core::ids::{CitizenId, OrgId};
use dsoc_core::{Authorization, Clock, Error, EventBus, Result};
use dsoc_db::Db;
use uuid::Uuid;

use crate::domain::{
    self, AdminOrg, AdminRole, FeatureFlag, RoleBinding, MAX_FLAG_KEY_LEN, MIN_MUTATION_LEVEL,
};
use crate::queries::{self, AdminOrgRow, FeatureFlagRow, RoleBindingRow};

/// Default page size for list endpoints when the caller does not specify one.
pub const DEFAULT_PAGE_LIMIT: u32 = 50;

/// Hard cap on page size, applied even if the caller requests more.
pub const MAX_PAGE_LIMIT: u32 = 100;

/// The largest accepted feature-flag key length, re-exported for callers building requests.
pub const FLAG_KEY_LIMIT: usize = MAX_FLAG_KEY_LEN;

/// System & organization administration service.
#[derive(Clone)]
pub struct AdminService {
    db: Db,
    clock: Arc<dyn Clock>,
    bus: Arc<dyn EventBus>,
    authz: Arc<dyn Authorization>,
}

impl std::fmt::Debug for AdminService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminService")
            .field("db", &"PgPool")
            .finish_non_exhaustive()
    }
}

impl AdminService {
    /// Construct from explicitly injected ports (used directly by integration tests).
    #[must_use]
    pub fn new(
        db: Db,
        clock: Arc<dyn Clock>,
        bus: Arc<dyn EventBus>,
        authz: Arc<dyn Authorization>,
    ) -> Self {
        Self {
            db,
            clock,
            bus,
            authz,
        }
    }

    /// Construct from the shared [`AppState`] the gateway injects (ADR-0004 wiring).
    #[must_use]
    pub fn from_state(state: &AppState) -> Self {
        Self {
            db: state.db.clone(),
            clock: Arc::clone(&state.clock),
            bus: Arc::clone(&state.bus),
            authz: Arc::clone(&state.authz),
        }
    }

    /// The injected event-bus publish port. Admin persists state but emits no cross-crate
    /// events for now (the frozen catalog has no `admin.*` variants); this accessor exposes
    /// the port the gateway wired so the contract is observable and future-proof.
    #[must_use]
    pub fn event_bus(&self) -> &Arc<dyn EventBus> {
        &self.bus
    }

    /// Resolve the caller's effective [`Permissions`] in `org` (R0.3 / ADR-0011): the union of
    /// every bound role's keys plus the implicit Base role. Pure read; the caller (gateway
    /// helper `require_permission`) decides whether a given key is required.
    ///
    /// # Errors
    /// [`Error::Storage`] on persistence failure.
    pub async fn permissions_for(
        &self,
        org: OrgId,
        citizen: CitizenId,
    ) -> Result<crate::permissions::Permissions> {
        let lists =
            queries::effective_permission_key_lists(&self.db, org.as_uuid(), citizen.as_uuid())
                .await
                .map_err(map_sqlx)?;
        Ok(crate::permissions::Permissions::from_role_key_lists(lists))
    }

    /// Assert the caller may perform an administrative mutation in `org`.
    ///
    /// SECURITY (2026-07-26, security queue R0.4 / ADR-0011): this used to require only
    /// `VerificationLevel::Directory` — ANY verified citizen could `bind_role` to themselves
    /// including the `owner` role. It now requires an `owner`|`admin` binding in `admin_role_binding`
    /// (root of trust: `scripts/bootstrap-admin.sh` seeds the first owner via SQL).
    /// The verification-level gate stays as defence in depth. Interim gate
    /// until R0.3's `RequirePermission`/`roles.manage`.
    async fn authorize_mutation(&self, org: OrgId, actor: CitizenId) -> Result<()> {
        self.authz.require(org, actor, MIN_MUTATION_LEVEL).await?;
        let is_admin = queries::actor_has_admin_role(&self.db, org.as_uuid(), actor.as_uuid())
            .await
            .map_err(map_sqlx)?;
        if is_admin {
            Ok(())
        } else {
            Err(Error::Forbidden(
                "requer papel de administrador nesta organização".to_owned(),
            ))
        }
    }

    /// Create the administrative extension for an existing baseline organization.
    ///
    /// # Errors
    /// [`Error::Forbidden`] if the actor is not authorized, [`Error::Conflict`] if the org
    /// is already administered, [`Error::Storage`] on other persistence failures.
    pub async fn create_org(&self, org: OrgId, actor: CitizenId) -> Result<AdminOrg> {
        self.authorize_mutation(org, actor).await?;
        let now = self.clock.now();
        let row = queries::insert_admin_org(&self.db, org.as_uuid(), now)
            .await
            .map_err(map_sqlx)?;
        Ok(row.into())
    }

    /// Fetch an administrative org. Reads are unrestricted.
    ///
    /// # Errors
    /// [`Error::NotFound`] if absent, [`Error::Storage`] on persistence failure.
    pub async fn get_org(&self, org: OrgId) -> Result<AdminOrg> {
        let row = queries::get_admin_org(&self.db, org.as_uuid())
            .await
            .map_err(map_sqlx)?;
        Ok(row.into())
    }

    /// List administrative orgs (keyset pagination over org id).
    ///
    /// # Errors
    /// [`Error::Storage`] on persistence failure.
    pub async fn list_orgs(
        &self,
        after: Option<OrgId>,
        limit: Option<u32>,
    ) -> Result<Vec<AdminOrg>> {
        let rows = queries::list_admin_orgs(
            &self.db,
            after.map(|org| org.as_uuid()),
            domain::clamp_limit(limit, MAX_PAGE_LIMIT),
        )
        .await
        .map_err(map_sqlx)?;
        Ok(rows.into_iter().map(AdminOrg::from).collect())
    }

    /// Bind an administrative role to a citizen within an org.
    ///
    /// # Errors
    /// [`Error::Forbidden`] if unauthorized, [`Error::Conflict`] on a duplicate grant,
    /// [`Error::Storage`] otherwise.
    pub async fn bind_role(
        &self,
        org: OrgId,
        actor: CitizenId,
        citizen: CitizenId,
        role: AdminRole,
    ) -> Result<RoleBinding> {
        self.authorize_mutation(org, actor).await?;
        let now = self.clock.now();
        let row = queries::insert_role_binding(
            &self.db,
            Uuid::now_v7(),
            org.as_uuid(),
            citizen.as_uuid(),
            role.as_str(),
            now,
        )
        .await
        .map_err(map_sqlx)?;
        map_role_binding(row)
    }

    /// List role bindings for an org (keyset pagination over binding id).
    ///
    /// # Errors
    /// [`Error::Storage`] on persistence failure.
    pub async fn list_role_bindings(
        &self,
        org: OrgId,
        after: Option<Uuid>,
        limit: Option<u32>,
    ) -> Result<Vec<RoleBinding>> {
        let rows = queries::list_role_bindings(
            &self.db,
            org.as_uuid(),
            after,
            domain::clamp_limit(limit, MAX_PAGE_LIMIT),
        )
        .await
        .map_err(map_sqlx)?;
        rows.into_iter().map(map_role_binding).collect()
    }

    /// Set a feature flag to `enabled`. Idempotent: repeating the same value leaves the
    /// flag in the same state (the `(org, key)` row is upserted, `updated_at` advanced).
    ///
    /// # Errors
    /// [`Error::Forbidden`] if unauthorized, [`Error::Validation`] on a malformed key,
    /// [`Error::Storage`] on persistence failure.
    pub async fn set_feature_flag(
        &self,
        org: OrgId,
        actor: CitizenId,
        key: &str,
        enabled: bool,
    ) -> Result<FeatureFlag> {
        self.authorize_mutation(org, actor).await?;
        domain::validate_flag_key(key)?;
        let now = self.clock.now();
        let row = queries::upsert_feature_flag(
            &self.db,
            Uuid::now_v7(),
            org.as_uuid(),
            key,
            enabled,
            now,
        )
        .await
        .map_err(map_sqlx)?;
        Ok(row.into())
    }

    /// Fetch a single feature flag.
    ///
    /// # Errors
    /// [`Error::Validation`] on a malformed key, [`Error::NotFound`] if absent,
    /// [`Error::Storage`] on persistence failure.
    pub async fn get_feature_flag(&self, org: OrgId, key: &str) -> Result<FeatureFlag> {
        domain::validate_flag_key(key)?;
        let row = queries::get_feature_flag(&self.db, org.as_uuid(), key)
            .await
            .map_err(map_sqlx)?;
        Ok(row.into())
    }

    /// List feature flags for an org (keyset pagination over flag id).
    ///
    /// # Errors
    /// [`Error::Storage`] on persistence failure.
    pub async fn list_feature_flags(
        &self,
        org: OrgId,
        after: Option<Uuid>,
        limit: Option<u32>,
    ) -> Result<Vec<FeatureFlag>> {
        let rows = queries::list_feature_flags(
            &self.db,
            org.as_uuid(),
            after,
            domain::clamp_limit(limit, MAX_PAGE_LIMIT),
        )
        .await
        .map_err(map_sqlx)?;
        Ok(rows.into_iter().map(FeatureFlag::from).collect())
    }
}

/// Map a raw `sqlx` failure into the canonical, public-safe [`Error`].
fn map_sqlx(err: sqlx::Error) -> Error {
    if matches!(err, sqlx::Error::RowNotFound) {
        return Error::NotFound("administrative record not found".to_owned());
    }
    if let sqlx::Error::Database(ref db_err) = err {
        if db_err.is_unique_violation() {
            return Error::Conflict("administrative record already exists".to_owned());
        }
    }
    tracing::error!(error = %err, "admin storage failure");
    Error::Storage(Box::new(err))
}

impl From<AdminOrgRow> for AdminOrg {
    fn from(row: AdminOrgRow) -> Self {
        Self {
            org_id: OrgId::from_uuid(row.org_id),
            is_active: row.is_active,
            created_at: row.created_at,
        }
    }
}

impl From<FeatureFlagRow> for FeatureFlag {
    fn from(row: FeatureFlagRow) -> Self {
        Self {
            id: row.id,
            org_id: OrgId::from_uuid(row.org_id),
            key: row.key,
            enabled: row.enabled,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Convert a role-binding row, treating an unrecognized stored role as a data-integrity
/// failure (the DB `CHECK` should prevent it) surfaced as [`Error::Storage`].
fn map_role_binding(row: RoleBindingRow) -> Result<RoleBinding> {
    let role = AdminRole::parse(&row.role).map_err(|_| {
        tracing::error!(role = %row.role, "unrecognized role stored in admin_role_binding");
        Error::Storage(Box::new(std::io::Error::other(
            "unrecognized administrative role in storage",
        )))
    })?;
    Ok(RoleBinding {
        id: row.id,
        org_id: OrgId::from_uuid(row.org_id),
        citizen_id: CitizenId::from_uuid(row.citizen_id),
        role,
        created_at: row.created_at,
    })
}

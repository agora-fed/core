//! `ZitadelAuth` — the concrete sovereign-identity service. Holds the `Db` pool plus the
//! injected `Arc<dyn Clock>` and `Arc<dyn EventBus>` (ADR-0004 wiring), validates OIDC tokens,
//! issues sessions, and manages graded verification levels. Implements
//! [`dsoc_core::Authorization`]. All `sqlx` failures are mapped onto the canonical
//! [`dsoc_core::Error`] model here (RowNotFound→NotFound, unique-violation→Conflict, else Storage).

use std::fmt;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use dsoc_core::events::EventEnvelope;
use dsoc_core::ids::{CitizenId, OrgId};
use dsoc_core::{Authorization, Clock, Error, EventBus, Result, VerificationLevel};
use dsoc_db::Db;

use crate::domain::{self, TokenValidator};
use crate::{events, queries};

/// Verification level granted to a citizen provisioned on first sovereign login. The token is
/// OIDC-validated (the subject proved control of an account), so `Email` is the floor.
const PROVISION_LEVEL: VerificationLevel = VerificationLevel::Email;

/// Hard cap on a verification-history page (unbounded reads must be bounded — PLAN.md).
const MAX_HISTORY_LIMIT: i64 = 100;

/// A session issued from a validated token exchange.
#[derive(Debug, Clone)]
pub struct IssuedSession {
    /// Opaque session id (UUIDv7, time-ordered).
    pub id: Uuid,
    /// The citizen this session authenticates.
    pub citizen: CitizenId,
    /// Sovereign OIDC subject bound to the session.
    pub oidc_subject: String,
    /// When the session was issued (injected clock).
    pub issued_at: DateTime<Utc>,
    /// When the session expires.
    pub expires_at: DateTime<Utc>,
    /// Stable public ActivityPub-readiness handle (ADR-0005).
    pub public_handle: String,
}

/// The resolved identity behind a presented token (`GET /auth/me`).
#[derive(Debug, Clone)]
pub struct Identity {
    /// The citizen.
    pub citizen: CitizenId,
    /// Sovereign OIDC subject.
    pub oidc_subject: String,
    /// Current verification level.
    pub level: VerificationLevel,
    /// Stable public ActivityPub-readiness handle (ADR-0005).
    pub public_handle: String,
}

/// One entry of the verification-level audit trail.
#[derive(Debug, Clone)]
pub struct AuditEntry {
    /// Audit row id.
    pub id: Uuid,
    /// Level before the change.
    pub old_level: VerificationLevel,
    /// Level after the change.
    pub new_level: VerificationLevel,
    /// When the change was recorded.
    pub changed_at: DateTime<Utc>,
}

/// Concrete sovereign-identity service.
pub struct ZitadelAuth {
    db: Db,
    clock: Arc<dyn Clock>,
    /// Injected event bus, retained per ADR-0006 for fire-and-forget emission **outside** a domain
    /// transaction. In-transaction emission (e.g. [`ZitadelAuth::upgrade_level`]) goes through the
    /// transactional outbox instead; this crate has no fire-and-forget path yet, hence `dead_code`.
    #[allow(dead_code)]
    bus: Arc<dyn EventBus>,
    validator: TokenValidator,
    session_ttl_secs: i64,
    /// Pluggable CPF verifier (algorithmic now; Serpro/KYC later) — ADR-0008.
    cpf_verifier: Arc<dyn crate::credential::CpfVerifier>,
}

impl fmt::Debug for ZitadelAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ZitadelAuth")
            .field("validator", &self.validator)
            .field("session_ttl_secs", &self.session_ttl_secs)
            .finish_non_exhaustive()
    }
}

/// Map a `sqlx` failure onto the canonical, public-safe error model.
fn map_sqlx(error: sqlx::Error) -> Error {
    match error {
        sqlx::Error::RowNotFound => Error::NotFound("entity not found".to_owned()),
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            Error::Conflict("entity already exists".to_owned())
        }
        other => Error::Storage(Box::new(other)),
    }
}

/// Map a `sqlx` failure during **registration** specifically: a unique-violation here is the user's
/// e-mail or CPF already being taken, and the http layer renders a precise message for each so the
/// person sees exactly which field collided (not a generic "conflito de estado"). The constraint
/// names come from migration `0101_auth_credentials.sql` (`UNIQUE (org_id, email)` and
/// `UNIQUE (org_id, cpf)` — PostgreSQL auto-names them `<table>_<cols>_key`).
fn map_register_sqlx(error: sqlx::Error) -> Error {
    if let sqlx::Error::Database(db) = &error {
        if db.is_unique_violation() {
            let constraint = db.constraint().unwrap_or("");
            if constraint.contains("email") {
                return Error::Conflict("email_taken".to_owned());
            }
            if constraint.contains("cpf") {
                return Error::Conflict("cpf_taken".to_owned());
            }
            return Error::Conflict("entity already exists".to_owned());
        }
    }
    map_sqlx(error)
}

/// Normalize and minimally validate an e-mail (trim + lowercase; must look like an address).
fn normalize_email(email: &str) -> Result<String> {
    let e = email.trim().to_lowercase();
    if e.len() < 3 || !e.contains('@') || !e.split('@').nth(1).is_some_and(|d| d.contains('.')) {
        return Err(Error::Validation("e-mail inválido".to_string()));
    }
    Ok(e)
}

impl ZitadelAuth {
    /// Construct the service from its injected collaborators.
    #[must_use]
    pub fn new(
        db: Db,
        clock: Arc<dyn Clock>,
        bus: Arc<dyn EventBus>,
        validator: TokenValidator,
        session_ttl_secs: i64,
    ) -> Self {
        Self {
            db,
            clock,
            bus,
            validator,
            session_ttl_secs,
            cpf_verifier: Arc::new(crate::credential::AlgorithmicCpfVerifier),
        }
    }

    /// Register a citizen with **e-mail + senha + CPF** (sovereign credential auth — ADR-0008). The
    /// CPF is validated (check digits) and its assurance recorded; the password is Argon2id-hashed;
    /// a session is issued. The whole flow runs in one transaction.
    ///
    /// # Errors
    /// [`Error::Validation`] for a bad e-mail/senha/CPF; [`Error::Conflict`] if the e-mail or CPF is
    /// already registered in the org.
    pub async fn register(
        &self,
        org: OrgId,
        email: &str,
        password: &str,
        cpf_raw: &str,
    ) -> Result<IssuedSession> {
        let now = self.clock.now();
        let email = normalize_email(email)?;
        let cpf = crate::credential::Cpf::parse(cpf_raw)?;
        let password_hash = crate::credential::hash_password(password)?;
        let cpf_status = self.cpf_verifier.verify(&cpf).await;
        let level = match cpf_status {
            crate::credential::CpfStatus::Verified => VerificationLevel::Strong,
            _ => VerificationLevel::Email,
        };

        let mut tx = self.db.begin().await.map_err(map_register_sqlx)?;
        let citizen = CitizenId::new();
        queries::insert_credential_citizen(
            &mut *tx,
            citizen.as_uuid(),
            org.as_uuid(),
            domain::level_as_str(level),
            now,
        )
        .await
        .map_err(map_register_sqlx)?;
        queries::insert_credential(
            &mut *tx,
            Uuid::now_v7(),
            citizen.as_uuid(),
            org.as_uuid(),
            &email,
            &password_hash,
            cpf.as_str(),
            cpf_status.as_str(),
            now,
        )
        .await
        .map_err(map_register_sqlx)?;

        let session = self
            .persist_session(&mut tx, org, citizen, &email, now)
            .await?;
        tx.commit().await.map_err(map_sqlx)?;
        Ok(session)
    }

    /// **Politician self-registration** (F1.3/F1.4). Same fields as [`Self::register`] plus a
    /// chosen `mandate_id`. Enforces `body.email == mandate.public_email` (case-insensitive) —
    /// the *only* proof of mandate control we can automatically verify at registration time. The
    /// same tx then (a) creates the citizen + credential, (b) sets `citizen.is_public=true`
    /// (mandates são públicos por transparência), and (c) records a
    /// `mandate_identity_binding` at level `directory`. On e-mail mismatch, returns
    /// [`Error::Conflict`] with a Portuguese message; the caller sees "credentials don't match"
    /// without any signal about whether the mandate exists (mitigates enumeration of officials).
    ///
    /// # Errors
    /// [`Error::Validation`] for bad e-mail/senha/CPF;
    /// [`Error::NotFound`] if the mandate does not exist in the org;
    /// [`Error::Conflict`] if the e-mail is already taken OR does not match `mandate.public_email`.
    pub async fn register_politician(
        &self,
        org: OrgId,
        email: &str,
        password: &str,
        cpf_raw: &str,
        mandate_id: Uuid,
    ) -> Result<IssuedSession> {
        let now = self.clock.now();
        let email = normalize_email(email)?;
        let cpf = crate::credential::Cpf::parse(cpf_raw)?;
        let password_hash = crate::credential::hash_password(password)?;

        // Look up the mandate FIRST — we need its public_email to gate the flow. Reading outside
        // the write tx is fine: the check is defense against user error, not a race against
        // concurrent mandate mutation (mandates change rarely and via admin flows).
        let mandate = queries::find_mandate_public_email(&self.db, org.as_uuid(), mandate_id)
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| {
                Error::Conflict("Mandato não encontrado ou e-mail não confere.".to_owned())
            })?;
        if mandate.to_lowercase() != email {
            // Same opaque error as "not found" so an attacker can't enumerate mandates whose
            // public_email is known: both paths converge on the same message.
            return Err(Error::Conflict(
                "Mandato não encontrado ou e-mail não confere.".to_owned(),
            ));
        }

        let cpf_status = self.cpf_verifier.verify(&cpf).await;
        let mut tx = self.db.begin().await.map_err(map_register_sqlx)?;
        let citizen = CitizenId::new();
        // Politicians land at `directory` verification level right away — the e-mail match is
        // the directory proof (the address is on the public registry maintained by the Câmara/
        // Senado/TSE). Higher levels (strong) still require CPF verification.
        queries::insert_credential_citizen(
            &mut *tx,
            citizen.as_uuid(),
            org.as_uuid(),
            domain::level_as_str(VerificationLevel::Directory),
            now,
        )
        .await
        .map_err(map_register_sqlx)?;
        queries::insert_credential(
            &mut *tx,
            Uuid::now_v7(),
            citizen.as_uuid(),
            org.as_uuid(),
            &email,
            &password_hash,
            cpf.as_str(),
            cpf_status.as_str(),
            now,
        )
        .await
        .map_err(map_register_sqlx)?;
        // Public by design — politicians on the platform ARE the accountability surface.
        queries::force_citizen_public(&mut *tx, citizen.as_uuid())
            .await
            .map_err(map_sqlx)?;
        // Record the binding at level `directory` inside the same tx so the register endpoint is
        // atomic: either the whole politician onboarding lands or nothing does.
        queries::insert_mandate_identity_binding(
            &mut *tx,
            Uuid::now_v7(),
            mandate_id,
            citizen.as_uuid(),
            "directory",
            None,
            now,
        )
        .await
        .map_err(map_sqlx)?;
        let session = self
            .persist_session(&mut tx, org, citizen, &email, now)
            .await?;
        tx.commit().await.map_err(map_sqlx)?;
        Ok(session)
    }

    /// Authenticate with **e-mail + senha** and issue a session.
    ///
    /// # Errors
    /// [`Error::Unauthorized`] for an unknown e-mail or a wrong password.
    pub async fn login(&self, org: OrgId, email: &str, password: &str) -> Result<IssuedSession> {
        let now = self.clock.now();
        let email = normalize_email(email)?;
        let cred = queries::find_credential_by_email(&self.db, org.as_uuid(), &email)
            .await
            .map_err(map_sqlx)?
            .ok_or(Error::Unauthorized)?;
        if !crate::credential::verify_password(password, &cred.password_hash) {
            return Err(Error::Unauthorized);
        }
        // 0.26.11: contas suspensas pela moderação (citizen.suspended_at) não
        // podem logar. Silenced continua logando; a UI é que esconde as notas
        // do feed público.
        let suspended: bool =
            sqlx::query_scalar(r"SELECT suspended_at IS NOT NULL FROM citizen WHERE id = $1")
                .bind(cred.citizen_id)
                .fetch_one(&self.db)
                .await
                .map_err(map_sqlx)?;
        if suspended {
            return Err(Error::Unauthorized);
        }
        // 0.28.2: contas em revisão (citizen.pending_review, migration 0514)
        // não logam até um admin aprovar em /admin/revisoes. Diferente do
        // suspended, a mensagem é explícita — a pessoa acertou a senha e
        // precisa saber o que está acontecendo com a própria conta.
        let pending: bool = sqlx::query_scalar(r"SELECT pending_review FROM citizen WHERE id = $1")
            .bind(cred.citizen_id)
            .fetch_one(&self.db)
            .await
            .map_err(map_sqlx)?;
        if pending {
            return Err(Error::Forbidden(
                "Sua conta foi criada e aguarda aprovação de um administrador.".to_owned(),
            ));
        }
        let citizen = CitizenId::from_uuid(cred.citizen_id);
        let mut tx = self.db.begin().await.map_err(map_sqlx)?;
        let session = self
            .persist_session(&mut tx, org, citizen, &email, now)
            .await?;
        tx.commit().await.map_err(map_sqlx)?;
        Ok(session)
    }

    /// Issue a session **within an already-open transaction**. Public sibling of
    /// [`Self::persist_session`] so external flows (e.g. `mandate_invite::accept`) can build a
    /// session as the terminal step of their own transactional writes without duplicating the
    /// cookie/expiry math. The caller owns the commit.
    ///
    /// # Errors
    /// [`Error::Storage`] on persistence failure.
    pub async fn issue_session_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        org: OrgId,
        citizen: CitizenId,
        email: &str,
        now: DateTime<Utc>,
    ) -> Result<IssuedSession> {
        self.persist_session(tx, org, citizen, email, now).await
    }

    /// Persist a session row (within an open tx) and return the issued session.
    async fn persist_session(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        org: OrgId,
        citizen: CitizenId,
        email: &str,
        now: DateTime<Utc>,
    ) -> Result<IssuedSession> {
        let subject = format!("cred:{email}");
        let handle = domain::public_handle(citizen);
        let session_id = Uuid::now_v7();
        let expires_at = domain::compute_expiry(now, self.session_ttl_secs);
        queries::insert_session(
            &mut **tx,
            session_id,
            org.as_uuid(),
            citizen.as_uuid(),
            &subject,
            now,
            expires_at,
            &handle,
        )
        .await
        .map_err(map_sqlx)?;
        Ok(IssuedSession {
            id: session_id,
            citizen,
            oidc_subject: subject,
            issued_at: now,
            expires_at,
            public_handle: handle,
        })
    }

    /// Invalidate a session by id (logout). Idempotent — missing/already-expired sessions are not
    /// an error: a stale browser tab signing out should never see a failure.
    ///
    /// # Errors
    /// [`Error::Storage`] on persistence failure.
    pub async fn delete_session(&self, session_id: Uuid) -> Result<()> {
        queries::delete_session(&self.db, session_id)
            .await
            .map_err(map_sqlx)?;
        Ok(())
    }

    /// Exchange a validated OIDC token for a server-issued session, provisioning the citizen on
    /// first login. The whole exchange runs in one transaction.
    ///
    /// # Errors
    /// [`Error::Unauthorized`] for an invalid/expired token; [`Error::Storage`] /
    /// [`Error::Conflict`] for persistence failures.
    pub async fn create_session(&self, org: OrgId, token: &str) -> Result<IssuedSession> {
        let now = self.clock.now();
        let validated = self.validator.validate(token, now).await?;

        let mut tx = self.db.begin().await.map_err(map_sqlx)?;

        let citizen_id =
            match queries::find_citizen_by_subject(&mut *tx, org.as_uuid(), &validated.subject)
                .await
                .map_err(map_sqlx)?
            {
                Some(row) => row.id,
                None => {
                    let fresh = CitizenId::new();
                    queries::insert_citizen(
                        &mut *tx,
                        fresh.as_uuid(),
                        org.as_uuid(),
                        &validated.subject,
                        domain::level_as_str(PROVISION_LEVEL),
                        now,
                    )
                    .await
                    .map_err(map_sqlx)?;
                    fresh.as_uuid()
                }
            };

        let citizen = CitizenId::from_uuid(citizen_id);
        let handle = domain::public_handle(citizen);
        let session_id = Uuid::now_v7();
        let expires_at = domain::compute_expiry(now, self.session_ttl_secs);

        queries::insert_session(
            &mut *tx,
            session_id,
            org.as_uuid(),
            citizen_id,
            &validated.subject,
            now,
            expires_at,
            &handle,
        )
        .await
        .map_err(map_sqlx)?;

        tx.commit().await.map_err(map_sqlx)?;

        Ok(IssuedSession {
            id: session_id,
            citizen,
            oidc_subject: validated.subject,
            issued_at: now,
            expires_at,
            public_handle: handle,
        })
    }

    /// Resolve the identity behind a presented token (`GET /auth/me`). Read-only.
    ///
    /// # Errors
    /// [`Error::Unauthorized`] for an invalid/expired token; [`Error::NotFound`] if no citizen is
    /// bound to the token's subject (no session was ever established).
    pub async fn me(&self, org: OrgId, token: &str) -> Result<Identity> {
        let now = self.clock.now();
        let validated = self.validator.validate(token, now).await?;

        let row = queries::find_citizen_by_subject(&self.db, org.as_uuid(), &validated.subject)
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| Error::NotFound("citizen not provisioned".to_owned()))?;

        let level = domain::level_from_str(&row.verification_level)?;
        let citizen = CitizenId::from_uuid(row.id);
        Ok(Identity {
            citizen,
            oidc_subject: validated.subject,
            level,
            public_handle: domain::public_handle(citizen),
        })
    }

    /// Raise a citizen's verification level. A genuine upgrade is audited and emits
    /// `auth.verification.upgraded`; a no-op or downgrade returns the current level silently.
    ///
    /// # Errors
    /// [`Error::NotFound`] if the citizen does not exist; [`Error::Storage`] on persistence failure.
    pub async fn upgrade_level(
        &self,
        org: OrgId,
        citizen: CitizenId,
        target: VerificationLevel,
    ) -> Result<VerificationLevel> {
        let now = self.clock.now();

        // Read-modify-write the level inside one transaction so two concurrent upgrades for the
        // same citizen cannot both append to the append-only audit trail (TOCTOU). The level read
        // used to sit outside the transaction; moving it inside under `SELECT ... FOR UPDATE`
        // serializes upgrades on the citizen row.
        let mut tx = self.db.begin().await.map_err(map_sqlx)?;

        let raw = queries::citizen_level_for_update(&mut *tx, org.as_uuid(), citizen.as_uuid())
            .await
            .map_err(map_sqlx)?;
        let current = domain::level_from_str(&raw)?;

        if !domain::is_upgrade(current, target) {
            // No-op or downgrade: nothing to persist; release the row lock.
            tx.rollback().await.map_err(map_sqlx)?;
            return Ok(current);
        }

        // Optimistic guard alongside the lock: the UPDATE only fires while the row still holds the
        // level we read. 0 rows means a concurrent writer already advanced it — treat as
        // already-at-target rather than appending a duplicate audit row / event.
        let affected = queries::update_citizen_level(
            &mut *tx,
            org.as_uuid(),
            citizen.as_uuid(),
            domain::level_as_str(current),
            domain::level_as_str(target),
        )
        .await
        .map_err(map_sqlx)?;
        if affected == 0 {
            tx.rollback().await.map_err(map_sqlx)?;
            return Ok(target);
        }

        queries::insert_verification_audit(
            &mut *tx,
            Uuid::now_v7(),
            org.as_uuid(),
            citizen.as_uuid(),
            domain::level_as_str(current),
            domain::level_as_str(target),
            now,
        )
        .await
        .map_err(map_sqlx)?;

        // Transactional outbox (ADR-0006): emit the event into `events_log` inside this same
        // transaction so the level change and its event commit atomically — no post-commit window
        // where the upgrade persisted but the event was lost (or a retry returned Conflict).
        let envelope = events::verification_upgraded_envelope(self.clock.as_ref(), org, citizen);
        dsoc_db::outbox::publish_tx(&mut *tx, &envelope)
            .await
            .map_err(map_sqlx)?;

        tx.commit().await.map_err(map_sqlx)?;
        Ok(target)
    }

    /// Keyset-paginated verification-level history for a citizen, newest first.
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn verification_history(
        &self,
        citizen: CitizenId,
        cursor: Option<(DateTime<Utc>, Uuid)>,
        limit: i64,
    ) -> Result<Vec<AuditEntry>> {
        let bounded = limit.clamp(1, MAX_HISTORY_LIMIT);
        let rows = queries::verification_history(&self.db, citizen.as_uuid(), cursor, bounded)
            .await
            .map_err(map_sqlx)?;
        rows.into_iter()
            .map(|r| {
                Ok(AuditEntry {
                    id: r.id,
                    old_level: domain::level_from_str(&r.old_level)?,
                    new_level: domain::level_from_str(&r.new_level)?,
                    changed_at: r.changed_at,
                })
            })
            .collect()
    }

    /// Handle an inbound cross-crate event (idempotent). Returns whether it was consumed.
    ///
    /// # Errors
    /// Propagates any error from the consume handler.
    pub fn consume(&self, envelope: &EventEnvelope) -> Result<bool> {
        events::on_event(envelope)
    }
}

#[async_trait::async_trait]
impl Authorization for ZitadelAuth {
    async fn level(&self, org: OrgId, citizen: CitizenId) -> Result<VerificationLevel> {
        let raw = queries::citizen_level(&self.db, org.as_uuid(), citizen.as_uuid())
            .await
            .map_err(map_sqlx)?;
        domain::level_from_str(&raw)
    }

    async fn require(
        &self,
        org: OrgId,
        citizen: CitizenId,
        required: VerificationLevel,
    ) -> Result<()> {
        let actual = self.level(org, citizen).await?;
        if actual >= required {
            Ok(())
        } else {
            Err(Error::Forbidden(format!(
                "verification level {} below required {}",
                domain::level_as_str(actual),
                domain::level_as_str(required),
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn map_sqlx_row_not_found_is_not_found() {
        let mapped = map_sqlx(sqlx::Error::RowNotFound);
        assert_eq!(mapped.code(), "not_found");
    }

    #[test]
    fn map_sqlx_protocol_error_is_storage() {
        let mapped = map_sqlx(sqlx::Error::Protocol("boom".into()));
        assert_eq!(mapped.code(), "storage_error");
        // Storage detail must not leak through Display.
        assert_eq!(mapped.to_string(), "storage error");
    }
}

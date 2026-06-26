//! The [`dsoc_core::traits::Space`] implementation for kind `"consultations"`. A consultation is a
//! participation *space* instance (its primary key is a `SpaceId`), so this crate fulfils the
//! frozen `Space` port: it names its kind, declares which components may be mounted into it, and
//! confirms a given consultation is open for participation. The open-check reads the consultation's
//! status straight from its own table — no peer crate is consulted (PLAN.md crate isolation).

use dsoc_core::ids::{OrgId, SpaceId};
use dsoc_core::traits::Space;
use dsoc_core::{Error, Result};
use dsoc_db::Db;

use crate::domain::ConsultationStatus;
use crate::queries;
use crate::service::map_sqlx;

/// Components that may be mounted into a consultation space. A consultation gathers structured
/// answers and discussion, so it admits `surveys` (its questions) and `comments` (deliberation).
const ALLOWED_COMPONENTS: &[&str] = &["surveys", "comments"];

/// The consultation space adapter over the `Db` pool. Cheaply cloneable.
#[derive(Clone)]
pub struct ConsultationSpace {
    db: Db,
}

impl std::fmt::Debug for ConsultationSpace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConsultationSpace").finish_non_exhaustive()
    }
}

impl ConsultationSpace {
    /// Construct the space adapter from the connection pool.
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Build the space adapter from the shared application state.
    #[must_use]
    pub fn from_state(state: &dsoc_app::AppState) -> Self {
        Self::new(state.db.clone())
    }
}

impl Space for ConsultationSpace {
    fn kind(&self) -> &'static str {
        "consultations"
    }

    fn allows_component(&self, component: &str) -> bool {
        ALLOWED_COMPONENTS.contains(&component)
    }

    async fn ensure_open(&self, org: OrgId, space: SpaceId) -> Result<()> {
        let raw = queries::consultation_status(&self.db, space.as_uuid(), org.as_uuid())
            .await
            .map_err(map_sqlx)?;
        let Some(raw) = raw else {
            return Err(Error::NotFound("consultation not found".to_owned()));
        };
        match ConsultationStatus::from_db(&raw) {
            Some(ConsultationStatus::Open) => Ok(()),
            Some(ConsultationStatus::Closed) => {
                Err(Error::Conflict("consultation is closed".to_owned()))
            }
            None => Err(Error::Storage("unknown consultation status in row".into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_components_are_exactly_surveys_and_comments() {
        assert_eq!(ALLOWED_COMPONENTS, &["surveys", "comments"]);
    }
}

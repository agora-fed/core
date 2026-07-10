//! The clustering service: holds the pool, the injected [`Clock`], and an [`Embedder`]. It embeds
//! each proposal, places it into the nearest cluster within the cosine threshold (or forms a new
//! one), and emits the matching catalog event through the **transactional outbox** (ADR-0006) so the
//! domain write and the event commit atomically. All failures map onto the canonical
//! [`dsoc_core::Error`].

use std::sync::Arc;

use dsoc_core::events::{Event, EventEnvelope};
use dsoc_core::ids::{ClusterId, OrgId, ProposalId};
use dsoc_core::{Clock, Error, Result};
use dsoc_db::Db;
use uuid::Uuid;

use crate::domain::{
    self, Decision, Embedder, Nearest, Placement, StubEmbedder, DEFAULT_THRESHOLD,
};
use crate::events;
use crate::queries::{self, ClusterRow};

/// Resolve the process-wide embedder from `CONSENSUS_EMBEDDER` (+ its default
/// threshold). The model branch caches the loaded instance in a `OnceLock`:
/// `from_state` is called once per worker subscription and per router, and the
/// artifacts are hundreds of MB — one load per process, ever.
fn embedder_from_env() -> (Arc<dyn Embedder>, f64) {
    let choice = std::env::var("CONSENSUS_EMBEDDER").unwrap_or_else(|_| "stub".to_owned());
    if !choice.eq_ignore_ascii_case("model") {
        return (Arc::new(StubEmbedder), DEFAULT_THRESHOLD);
    }
    #[cfg(feature = "model-embedder")]
    {
        use std::sync::OnceLock;
        static MODEL: OnceLock<Option<Arc<crate::model_embedder::ModelEmbedder>>> = OnceLock::new();
        let cached = MODEL.get_or_init(|| {
            let dir =
                std::env::var("CONSENSUS_MODEL_DIR").unwrap_or_else(|_| "/srv/model".to_owned());
            match crate::model_embedder::ModelEmbedder::load(std::path::Path::new(&dir)) {
                Ok(m) => Some(Arc::new(m)),
                Err(err) => {
                    tracing::error!(
                        dir,
                        error = %err,
                        "CONSENSUS_EMBEDDER=model but the model failed to load; \
                         FALLING BACK TO THE STUB — clustering is NOT semantic"
                    );
                    None
                }
            }
        });
        if let Some(model) = cached {
            return (
                model.clone(),
                crate::model_embedder::MODEL_DEFAULT_THRESHOLD,
            );
        }
    }
    #[cfg(not(feature = "model-embedder"))]
    tracing::error!(
        "CONSENSUS_EMBEDDER=model but this binary was compiled without the \
         `model-embedder` feature; FALLING BACK TO THE STUB"
    );
    (Arc::new(StubEmbedder), DEFAULT_THRESHOLD)
}

/// Semantic clustering service for proposals.
#[derive(Clone)]
pub struct ClusterService {
    db: Db,
    clock: Arc<dyn Clock>,
    embedder: Arc<dyn Embedder>,
    threshold: f64,
}

impl std::fmt::Debug for ClusterService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClusterService")
            .field("threshold", &self.threshold)
            .finish_non_exhaustive()
    }
}

impl ClusterService {
    /// Build a service with an explicit embedder and threshold (used by tests). Events are emitted
    /// through the transactional outbox (ADR-0006), so no [`dsoc_core::EventBus`] is injected here.
    #[must_use]
    pub fn new(db: Db, clock: Arc<dyn Clock>, embedder: Arc<dyn Embedder>, threshold: f64) -> Self {
        Self {
            db,
            clock,
            embedder,
            threshold,
        }
    }

    /// Build a service from the shared application state. The embedder and the
    /// merge threshold come from the environment:
    ///
    /// - `CONSENSUS_EMBEDDER` — `stub` (default; deterministic feature hashing)
    ///   or `model` (real local semantics via the `model-embedder` feature).
    /// - `CONSENSUS_MODEL_DIR` — artifacts directory for `model`.
    /// - `CONSENSUS_THRESHOLD` — cosine-distance override; defaults to
    ///   [`DEFAULT_THRESHOLD`] for the stub and to the calibrated
    ///   `model_embedder::MODEL_DEFAULT_THRESHOLD` for the model (E5-family
    ///   distances are compressed; see the calibration test).
    ///
    /// The model loads ONCE per process (it is hundreds of MB); every
    /// subsequent `from_state` reuses the cached instance. A requested-but-
    /// unloadable model falls back to the stub LOUDLY — the consequence loop
    /// must keep running even if someone fat-fingers the model path.
    #[must_use]
    pub fn from_state(state: &dsoc_app::AppState) -> Self {
        let (embedder, default_threshold) = embedder_from_env();
        let threshold = std::env::var("CONSENSUS_THRESHOLD")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|t| (0.0..=1.0).contains(t))
            .unwrap_or(default_threshold);
        Self::new(state.db.clone(), state.clock.clone(), embedder, threshold)
    }

    /// Embed `text`, then either merge `proposal` into its nearest cluster (within the cosine
    /// threshold) or seed a new cluster. Emits `consensus.proposal.merged` or
    /// `consensus.cluster.formed` through the transactional outbox **inside the same transaction**
    /// as the write, so the domain change and the event commit atomically (ADR-0006): there is no
    /// post-commit window where a committed placement could lose its event, and a retry that hits
    /// the `proposal_id` UNIQUE constraint cannot suppress an already-emitted signal.
    ///
    /// # Errors
    /// - [`Error::Validation`] when `text` is empty.
    /// - [`Error::Conflict`] when the proposal was already ingested (unique violation).
    /// - [`Error::Storage`] on any other persistence failure.
    pub async fn ingest(&self, org: OrgId, proposal: ProposalId, text: &str) -> Result<Placement> {
        domain::validate_text(text)?;
        let embedding = self.embedder.embed(text);
        let literal = domain::to_pgvector_literal(&embedding);
        let now = self.clock.now();

        let mut tx = self.db.begin().await.map_err(map_sqlx)?;

        queries::insert_embedding(&mut *tx, Uuid::now_v7(), proposal.as_uuid(), &literal, now)
            .await
            .map_err(map_sqlx)?;

        let nearest = queries::nearest_cluster(&mut *tx, org.as_uuid(), &literal)
            .await
            .map_err(map_sqlx)?
            .map(|row| Nearest {
                cluster: ClusterId::from_uuid(row.id),
                distance: row.distance,
            });

        let placement = match Decision::decide(nearest, self.threshold) {
            Decision::Merge { cluster, distance } => {
                queries::insert_member(
                    &mut *tx,
                    cluster.as_uuid(),
                    proposal.as_uuid(),
                    distance as f32,
                )
                .await
                .map_err(map_sqlx)?;
                let size = queries::recompute_centroid(&mut *tx, cluster.as_uuid())
                    .await
                    .map_err(map_sqlx)?;
                let envelope = events::envelope(
                    &self.clock,
                    org,
                    Event::ConsensusProposalMerged { proposal, cluster },
                );
                dsoc_db::outbox::publish_tx(&mut *tx, &envelope)
                    .await
                    .map_err(map_sqlx)?;
                tx.commit().await.map_err(map_sqlx)?;
                Placement::Merged {
                    cluster,
                    size: u32::try_from(size).unwrap_or_default(),
                }
            }
            Decision::Form => {
                let cluster = ClusterId::new();
                queries::insert_cluster(&mut *tx, cluster.as_uuid(), org.as_uuid(), &literal, now)
                    .await
                    .map_err(map_sqlx)?;
                queries::insert_member(&mut *tx, cluster.as_uuid(), proposal.as_uuid(), 0.0)
                    .await
                    .map_err(map_sqlx)?;
                let envelope = events::envelope(
                    &self.clock,
                    org,
                    Event::ConsensusClusterFormed { cluster, size: 1 },
                );
                dsoc_db::outbox::publish_tx(&mut *tx, &envelope)
                    .await
                    .map_err(map_sqlx)?;
                tx.commit().await.map_err(map_sqlx)?;
                Placement::Formed { cluster, proposal }
            }
        };
        Ok(placement)
    }

    /// Consume a `proposals.created` envelope: embed and cluster the proposal carried by it. The
    /// proposal body (`text`) is delivered alongside the slim event by the caller. Non-matching
    /// envelopes are ignored (idempotent no-op), returning `None`.
    ///
    /// # Errors
    /// Mirrors [`Self::ingest`].
    pub async fn consume(&self, envelope: &EventEnvelope, text: &str) -> Result<Option<Placement>> {
        match events::proposal_created(envelope) {
            Some((proposal, _mandate)) => {
                Ok(Some(self.ingest(envelope.org, proposal, text).await?))
            }
            None => Ok(None),
        }
    }

    /// Fetch a single cluster.
    ///
    /// # Errors
    /// [`Error::NotFound`] when the cluster does not exist; [`Error::Storage`] otherwise.
    pub async fn cluster(&self, id: ClusterId) -> Result<ClusterRow> {
        queries::get_cluster(&self.db, id.as_uuid())
            .await
            .map_err(map_sqlx)
    }

    /// List an org's clusters with keyset pagination, returning the rows and the total count.
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn list_clusters(
        &self,
        org: OrgId,
        after: Option<ClusterId>,
        limit: i64,
    ) -> Result<(Vec<ClusterRow>, i64)> {
        let limit = limit.clamp(1, 100);
        let rows =
            queries::list_clusters(&self.db, org.as_uuid(), after.map(|c| c.as_uuid()), limit)
                .await
                .map_err(map_sqlx)?;
        let total = queries::count_clusters(&self.db, org.as_uuid())
            .await
            .map_err(map_sqlx)?;
        Ok((rows, total))
    }
}

/// Map a `sqlx` failure onto the canonical error model (CONTRIBUTING.md wiring conventions):
/// missing row -> `NotFound`, unique violation -> `Conflict`, everything else -> `Storage`.
fn map_sqlx(err: sqlx::Error) -> Error {
    match err {
        sqlx::Error::RowNotFound => Error::NotFound("cluster not found".to_string()),
        sqlx::Error::Database(ref db) if db.is_unique_violation() => {
            Error::Conflict("proposal already clustered".to_string())
        }
        other => Error::Storage(Box::new(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_sqlx_row_not_found_is_not_found() {
        let e = map_sqlx(sqlx::Error::RowNotFound);
        assert_eq!(e.code(), "not_found");
    }

    #[test]
    fn map_sqlx_other_is_storage() {
        let e = map_sqlx(sqlx::Error::PoolTimedOut);
        assert_eq!(e.code(), "storage_error");
    }
}

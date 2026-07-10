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
    self, Decision, Embedder, Nearest, PairJudge, Placement, StubEmbedder, DEFAULT_THRESHOLD,
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

/// Resolve the optional NLI pair judge from `CONSENSUS_NLI_DIR`. Same
/// OnceLock posture as the embedder: the artifacts are ~1 GB in RAM, one load
/// per process. Unset env → `None`; a set-but-unloadable dir logs loudly and
/// returns `None` (distance + stance still guard the merge).
fn judge_from_env() -> Option<Arc<dyn PairJudge>> {
    let dir = std::env::var("CONSENSUS_NLI_DIR").ok()?;
    #[cfg(feature = "model-embedder")]
    {
        use std::sync::OnceLock;
        static JUDGE: OnceLock<Option<Arc<crate::nli_judge::NliJudge>>> = OnceLock::new();
        let cached = JUDGE.get_or_init(|| {
            match crate::nli_judge::NliJudge::load(std::path::Path::new(&dir)) {
                Ok(j) => Some(Arc::new(j)),
                Err(err) => {
                    tracing::error!(
                        dir,
                        error = %err,
                        "CONSENSUS_NLI_DIR set but the NLI judge failed to load; \
                         merges fall back to distance + stance lexicon"
                    );
                    None
                }
            }
        });
        cached.clone().map(|j| j as Arc<dyn PairJudge>)
    }
    #[cfg(not(feature = "model-embedder"))]
    {
        tracing::error!(
            dir,
            "CONSENSUS_NLI_DIR set but this binary was compiled without the \
             `model-embedder` feature; merges fall back to distance + stance lexicon"
        );
        None
    }
}

/// Semantic clustering service for proposals.
#[derive(Clone)]
pub struct ClusterService {
    db: Db,
    clock: Arc<dyn Clock>,
    embedder: Arc<dyn Embedder>,
    threshold: f64,
    /// Optional NLI pair judge (env `CONSENSUS_NLI_DIR`): reads merge
    /// candidates jointly. `None` (tests, judge unavailable) = distance +
    /// stance lexicon only.
    judge: Option<Arc<dyn PairJudge>>,
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
            judge: None,
        }
    }

    /// Attach a pair judge (see [`PairJudge`]); builder-style so `new` keeps
    /// its deterministic no-model shape for tests.
    #[must_use]
    pub fn with_judge(mut self, judge: Arc<dyn PairJudge>) -> Self {
        self.judge = Some(judge);
        self
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
        let svc = Self::new(state.db.clone(), state.clock.clone(), embedder, threshold);
        match judge_from_env() {
            Some(judge) => svc.with_judge(judge),
            None => svc,
        }
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
        let signature = crate::stance::direction_signature(text);
        let now = self.clock.now();

        let mut tx = self.db.begin().await.map_err(map_sqlx)?;

        // Sample the text for future NLI pair-judging (crate-owned copy,
        // like the signature; 1200 chars cover any civic ask's substance).
        let text_sample: String = text.chars().take(1200).collect();
        queries::insert_embedding(
            &mut *tx,
            Uuid::now_v7(),
            proposal.as_uuid(),
            &literal,
            &signature,
            &text_sample,
            now,
        )
        .await
        .map_err(map_sqlx)?;

        let nearest = queries::nearest_cluster(&mut *tx, org.as_uuid(), &literal)
            .await
            .map_err(map_sqlx)?
            .map(|row| Nearest {
                cluster: ClusterId::from_uuid(row.id),
                distance: row.distance,
            });

        let mut decision = Decision::decide(nearest, self.threshold);
        // Stance veto: the embedding finds "same topic"; only direction tells
        // "privatizar o SUS" apart from "proibir a privatização do SUS"
        // (measured cosine 0.015 — see stance.rs). An antagonistic direction
        // in the candidate cluster forces a NEW cluster instead of a merge.
        if let Decision::Merge { cluster, distance } = decision {
            let members = queries::member_signatures(&mut *tx, cluster.as_uuid(), 50)
                .await
                .map_err(map_sqlx)?;
            if members
                .iter()
                .any(|m| crate::stance::directions_conflict(&signature, m))
            {
                tracing::info!(
                    proposal = %proposal,
                    vetoed_cluster = %cluster,
                    distance,
                    "stance veto: antagonistic policy direction — forming a new cluster"
                );
                decision = Decision::Form;
            }
        }
        // NLI pair judge: distance says "same topic", the judge says whether
        // it is the SAME ASK — homonyms ("obra do mestre Picasso" vs "mestre
        // de obras", cosine 0.068) and different-scope asks read as Neutral
        // and must not merge. Fail-open: a judge error is "no opinion", the
        // cheaper guards above already had their say.
        if let (Decision::Merge { cluster, distance }, Some(judge)) = (decision, &self.judge) {
            let members = queries::member_texts(&mut *tx, cluster.as_uuid(), 3)
                .await
                .map_err(map_sqlx)?;
            for member in &members {
                match judge.same_ask(&text_sample, member) {
                    Ok(true) => {}
                    Ok(false) => {
                        tracing::info!(
                            proposal = %proposal,
                            vetoed_cluster = %cluster,
                            distance,
                            "nli veto: same topic but not the same ask — forming a new cluster"
                        );
                        decision = Decision::Form;
                        break;
                    }
                    Err(err) => {
                        tracing::warn!(
                            proposal = %proposal,
                            error = %err,
                            "nli judge failed; proceeding on distance + stance only"
                        );
                        break;
                    }
                }
            }
        }

        let placement = match decision {
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

    /// Backlog do re-embed (fatia 2a): propostas embedadas antes do 0518 —
    /// vetor da era stub e/ou sem amostra NLI. O composition root busca o
    /// texto (tabela de outro crate) e chama [`Self::re_embed`].
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn stale_backlog(&self, limit: i64) -> Result<Vec<ProposalId>> {
        let ids = queries::stale_embedding_proposals(&self.db, limit)
            .await
            .map_err(map_sqlx)?;
        Ok(ids.into_iter().map(ProposalId::from_uuid).collect())
    }

    /// Fatia 2a do re-cluster (0.28.4): regrava o embedding de uma proposta
    /// da era do stub com o modelo real, a assinatura de direção (stance.rs)
    /// e a amostra NLI, e recomputa o centroide do cluster onde ela vive.
    /// NÃO move a proposta de cluster — reavaliar membership (com skip de
    /// clusters que já dispararam SLA) é a fatia 2b, porque mover emite
    /// eventos e mexe no gatilho de threshold. Idempotente: a amostra
    /// preenchida tira a row do backlog de [`Self::stale_backlog`].
    ///
    /// # Errors
    /// [`Error::Validation`] pra texto vazio; [`Error::Storage`] em falha
    /// de persistência.
    pub async fn re_embed(&self, proposal: ProposalId, text: &str) -> Result<Option<ClusterId>> {
        domain::validate_text(text)?;
        let embedding = self.embedder.embed(text);
        let literal = domain::to_pgvector_literal(&embedding);
        let signature = crate::stance::direction_signature(text);
        let text_sample: String = text.chars().take(1200).collect();

        let mut tx = self.db.begin().await.map_err(map_sqlx)?;
        let updated = queries::update_embedding(
            &mut *tx,
            proposal.as_uuid(),
            &literal,
            &signature,
            &text_sample,
        )
        .await
        .map_err(map_sqlx)?;
        if !updated {
            // Proposta sem embedding (nunca ingerida) — nada a re-embedar.
            return Ok(None);
        }
        let cluster = queries::cluster_of_proposal(&mut *tx, proposal.as_uuid())
            .await
            .map_err(map_sqlx)?;
        if let Some(cluster) = cluster {
            queries::recompute_centroid(&mut *tx, cluster)
                .await
                .map_err(map_sqlx)?;
        }
        tx.commit().await.map_err(map_sqlx)?;
        Ok(cluster.map(ClusterId::from_uuid))
    }

    /// Purga a embedding de uma proposta que não existe mais (apagada por
    /// purge de demo ou LGPD art. 18): remove o edge de membership e a
    /// embedding, e recomputa o centroide do cluster — ou dissolve o
    /// cluster se ficou vazio. Chamado pelo composition root quando o
    /// fetch do texto retorna NotFound durante o re-embed (fatia 2a);
    /// sem isso a órfã fica em retry eterno no backlog.
    ///
    /// # Errors
    /// [`Error::Storage`] on a persistence failure.
    pub async fn purge_orphan(&self, proposal: ProposalId) -> Result<()> {
        let mut tx = self.db.begin().await.map_err(map_sqlx)?;
        let cluster = queries::delete_member(&mut *tx, proposal.as_uuid())
            .await
            .map_err(map_sqlx)?;
        queries::delete_embedding(&mut *tx, proposal.as_uuid())
            .await
            .map_err(map_sqlx)?;
        if let Some(cluster) = cluster {
            let dissolved = queries::delete_cluster_if_empty(&mut *tx, cluster)
                .await
                .map_err(map_sqlx)?;
            if !dissolved {
                queries::recompute_centroid(&mut *tx, cluster)
                    .await
                    .map_err(map_sqlx)?;
            }
        }
        tx.commit().await.map_err(map_sqlx)?;
        Ok(())
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

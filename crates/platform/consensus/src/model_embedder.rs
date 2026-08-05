//! Real semantic embeddings, locally (PLAN.md principle 11: sovereign models only).
//!
//! Loads a BERT-family sentence-embedding model from a local directory and runs
//! CPU inference through `candle` (pure Rust — no Python, no C runtime, no
//! network at inference time).
//!
//! Reference deployment: `sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2`
//! (384 dims — matches `EMBEDDING_DIM` and the `vector(384)` columns of migration
//! 0130). A PARAPHRASE model is deliberately chosen over a retrieval model
//! (e5-family): clustering asks "is this the same demand in other words?", and on
//! pt-BR civic pairs e5-small ranked "ciclovia na Av. X" closer to "recapear a
//! Av. X" (distinct asks, same place) than to true paraphrases — retrieval
//! models weigh topic/entity; paraphrase models weigh intent. See the
//! calibration test at the bottom.
//!
//! Model-agnostic mechanics: mean pooling over the attention mask, then L2
//! normalisation, so pgvector's `<=>` cosine distance behaves exactly like the
//! stub's output scale. `CONSENSUS_QUERY_PREFIX` supports prefix-requiring
//! families (E5: `"query: "`).
//!
//! Failure policy: `Embedder::embed` is infallible by contract (the clustering
//! pipeline must never wedge on one bad text), so an inference error logs
//! loudly and falls back to the deterministic [`StubEmbedder`] for THAT text —
//! degraded clustering beats a stalled consequence loop. Load errors are
//! surfaced at construction, where the caller can refuse to start.

use std::path::Path;
use std::sync::Mutex;

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config};
use tokenizers::{PaddingParams, Tokenizer, TruncationParams};

use crate::domain::{Embedder, StubEmbedder, EMBEDDING_DIM};

/// Max input tokens. E5-small was trained at 512; proposals are short civic
/// texts, so truncation is a safety net, not a lossy path.
const MAX_TOKENS: usize = 512;

/// Default merge threshold for the REAL model (reference: e5-small with
/// `CONSENSUS_QUERY_PREFIX="query: "`). E5 cosine distances are compressed:
/// on the pt-BR calibration pairs below, clear paraphrases land at 0.079–0.089
/// and clearly distinct asks at ≥ 0.134, so 0.10 splits them. Two measured
/// limitations are accepted and documented in the calibration test (they gate
/// nothing): the UBS/"posto de saúde" acronym pair misses the merge (0.111),
/// and "same street, different intervention" pairs can false-merge (0.078) —
/// both tracked for the V2 model upgrade. Overridable via CONSENSUS_THRESHOLD.
pub const MODEL_DEFAULT_THRESHOLD: f64 = 0.10;

/// A local BERT-family embedding model. Construction validates the artifacts;
/// inference is CPU-bound and takes the interior lock (candle tensors are not
/// `Sync`-shareable mid-forward; proposal ingest volume is low, so a mutex is
/// simpler and safer than a pool).
pub struct ModelEmbedder {
    inner: Mutex<Inner>,
    fallback: StubEmbedder,
}

struct Inner {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
}

impl std::fmt::Debug for ModelEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelEmbedder").finish_non_exhaustive()
    }
}

impl ModelEmbedder {
    /// Load `config.json` + `tokenizer.json` + `model.safetensors` from `dir`.
    ///
    /// # Errors
    /// A human-readable message when any artifact is missing/corrupt or the
    /// model's hidden size cannot produce [`EMBEDDING_DIM`]-length vectors.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let config_raw = std::fs::read_to_string(dir.join("config.json"))
            .map_err(|e| format!("config.json: {e}"))?;
        let config: Config =
            serde_json::from_str(&config_raw).map_err(|e| format!("config.json parse: {e}"))?;

        let mut tokenizer = Tokenizer::from_file(dir.join("tokenizer.json"))
            .map_err(|e| format!("tokenizer.json: {e}"))?;
        let _ = tokenizer.with_truncation(Some(TruncationParams {
            max_length: MAX_TOKENS,
            ..Default::default()
        }));
        tokenizer.with_padding(Some(PaddingParams::default()));

        let device = Device::Cpu;
        // Safe (buffered) loader — the workspace forbids `unsafe`, so no mmap.
        let weights = std::fs::read(dir.join("model.safetensors"))
            .map_err(|e| format!("model.safetensors: {e}"))?;
        let vb = VarBuilder::from_buffered_safetensors(weights, DType::F32, &device)
            .map_err(|e| format!("safetensors load: {e}"))?;
        let model = BertModel::load(vb, &config).map_err(|e| format!("bert load: {e}"))?;

        // Fail construction, not inference, on a dimensionality mismatch.
        let probe = Self::forward_inner(&model, &tokenizer, &device, "probe")
            .map_err(|e| format!("probe inference: {e}"))?;
        if probe.len() != EMBEDDING_DIM {
            return Err(format!(
                "model emits {} dims; the schema (migration 0130) requires {EMBEDDING_DIM}",
                probe.len()
            ));
        }

        tracing::info!(dir = %dir.display(), "consensus model embedder loaded (real semantics ON)");
        Ok(Self {
            inner: Mutex::new(Inner {
                model,
                tokenizer,
                device,
            }),
            fallback: StubEmbedder,
        })
    }

    fn forward_inner(
        model: &BertModel,
        tokenizer: &Tokenizer,
        device: &Device,
        text: &str,
    ) -> Result<Vec<f32>, candle_core::Error> {
        // Some model families require an instruction prefix (E5: "query: " on
        // both sides of a symmetric comparison); paraphrase models use none.
        // Empty default — set CONSENSUS_QUERY_PREFIX when deploying an E5.
        let prefix = std::env::var("CONSENSUS_QUERY_PREFIX").unwrap_or_default();
        let prompt = format!("{prefix}{text}");
        let encoding = tokenizer
            .encode(prompt, true)
            .map_err(candle_core::Error::msg)?;
        let ids = Tensor::new(encoding.get_ids(), device)?.unsqueeze(0)?;
        let type_ids = ids.zeros_like()?;
        let mask = Tensor::new(encoding.get_attention_mask(), device)?.unsqueeze(0)?;
        // (1, seq, hidden)
        let hidden = model.forward(&ids, &type_ids, Some(&mask))?;

        // Mean pooling over real (non-pad) tokens.
        let mask_f = mask.to_dtype(DType::F32)?.unsqueeze(2)?; // (1, seq, 1)
        let summed = hidden.broadcast_mul(&mask_f)?.sum(1)?; // (1, hidden)
        let counts = mask_f.sum(1)?.clamp(1e-9, f64::INFINITY)?; // (1, 1)
        let mean = summed.broadcast_div(&counts)?;

        let mut v = mean.squeeze(0)?.to_vec1::<f32>()?;
        // L2 normalise so cosine distance is directly comparable to the stub's.
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in &mut v {
                *x /= norm;
            }
        }
        Ok(v)
    }
}

impl Embedder for ModelEmbedder {
    fn embed(&self, text: &str) -> Vec<f32> {
        let guard = match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(), // model is stateless per call
        };
        match Self::forward_inner(&guard.model, &guard.tokenizer, &guard.device, text) {
            Ok(v) => v,
            Err(err) => {
                tracing::error!(
                    error = %err,
                    "model embedder inference failed; falling back to stub for this text"
                );
                self.fallback.embed(text)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ModelEmbedder, MODEL_DEFAULT_THRESHOLD};
    use crate::domain::{cosine_distance, Embedder};
    use std::path::Path;

    /// Semantic sanity — runs only where the model artifacts exist (the dev
    /// machine / a runner with CONSENSUS_MODEL_DIR set). CI without the env
    /// var skips silently: the contract under test is the MODEL's, and CI
    /// must not depend on a 470 MB artifact (TESTING.md: no live models).
    #[test]
    fn near_duplicates_cluster_closer_than_unrelated_texts() {
        let Ok(dir) = std::env::var("CONSENSUS_MODEL_DIR") else {
            eprintln!("CONSENSUS_MODEL_DIR unset; skipping model semantics test");
            return;
        };
        let embedder = ModelEmbedder::load(Path::new(&dir)).expect("load model");
        let d = |x: &str, y: &str| cosine_distance(&embedder.embed(x), &embedder.embed(y));

        // GATE 1 — clear paraphrases (same ask, near-zero token overlap) MUST
        // fall under the merge threshold. This is exactly what the FNV stub
        // could never do.
        let paraphrases = [
            d(
                "Precisamos de uma creche no bairro para as crianças",
                "Faltam vagas em berçário e creche para os bebês da região",
            ),
            d(
                "Mais ônibus no horário de pico",
                "Aumentar a frequência do transporte público pela manhã",
            ),
        ];
        // GATE 2 — clearly distinct asks MUST stay above the threshold.
        let distinct = [
            d(
                "Precisamos de uma creche no bairro para as crianças",
                "Recapear o asfalto da avenida principal que está cheio de buracos",
            ),
            d(
                "Precisamos de um posto de saúde na nossa região",
                "Mais policiamento e iluminação na praça central",
            ),
        ];
        // KNOWN LIMITATIONS (measured 2026-07-10 on e5-small; logged, not
        // gated — both tracked for the V2 model upgrade, larger model + intent
        // guard): domain acronyms miss the merge (UBS ≙ health post ≈ 0.111,
        // just above threshold); "same street, different intervention" can
        // false-merge (ciclovia vs recapear a mesma avenida ≈ 0.078).
        let acronym = d(
            "Construir uma UBS no bairro",
            "Precisamos de um posto de saúde na nossa região",
        );
        let same_place = d(
            "Construir uma ciclovia na avenida Brasil",
            "Recapear o asfalto da avenida Brasil",
        );

        // Printed on purpose: calibration data for CONSENSUS_THRESHOLD
        // (run with `-- --nocapture`).
        eprintln!("paraphrases={paraphrases:.4?} distinct={distinct:.4?}");
        eprintln!("known limits: acronym={acronym:.4} same_place={same_place:.4}");

        for (i, dist) in paraphrases.iter().enumerate() {
            assert!(
                *dist < MODEL_DEFAULT_THRESHOLD,
                "paraphrase pair {i} would NOT merge: {dist:.3} >= {MODEL_DEFAULT_THRESHOLD}"
            );
        }
        for (i, dist) in distinct.iter().enumerate() {
            assert!(
                *dist > MODEL_DEFAULT_THRESHOLD,
                "distinct pair {i} would FALSELY merge: {dist:.3} <= {MODEL_DEFAULT_THRESHOLD}"
            );
        }
    }

    /// The hardest class for ANY topic-oriented embedder: proposals that share
    /// the moral goal ("mais saúde") but are IDEOLOGICALLY ANTAGONISTIC in the
    /// means — merging them poisons the consensus signal (the SLA would fire
    /// for a self-contradictory demand a politician can rightly dismiss).
    /// Includes direct-negation pairs, where embedders are notoriously weak.
    /// Prints the full matrix (calibration data); gates assert the deployed
    /// threshold does not merge them.
    #[test]
    fn antagonistic_asks_must_not_merge() {
        let Ok(dir) = std::env::var("CONSENSUS_MODEL_DIR") else {
            eprintln!("CONSENSUS_MODEL_DIR unset; skipping model semantics test");
            return;
        };
        let embedder = ModelEmbedder::load(Path::new(&dir)).expect("load model");
        let d = |x: &str, y: &str| cosine_distance(&embedder.embed(x), &embedder.embed(y));

        // Same moral goal (health), opposite means: privatize vs strengthen
        // the public system.
        let ideological = d(
            "Quero mais saúde para o brasileiro vendendo o SUS para empresas competentes",
            "Precisamos subir os salários dos radiologistas que operam as máquinas de raio-x nos postos de saúde",
        );
        // Direct negation — near-identical tokens, inverted policy.
        let negation = d(
            "Privatizar a gestão dos postos de saúde do SUS",
            "Proibir a privatização dos postos de saúde do SUS",
        );
        // Antagonistic budget direction, same area.
        let budget = d(
            "Reduzir o orçamento da saúde pública para cortar impostos",
            "Aumentar o orçamento da saúde pública mesmo que os impostos subam",
        );
        // Control: same stance in different words — this one SHOULD merge.
        let control = d(
            "Contratar mais médicos para os postos de saúde",
            "Precisamos de mais profissionais de medicina nas unidades de saúde do bairro",
        );

        eprintln!(
            "antagonistic: ideological={ideological:.4} negation={negation:.4} budget={budget:.4} | control-paraphrase={control:.4}"
        );

        // LAYERED defense: a pair is protected when the embedding keeps it
        // above the threshold OR the stance guard (stance.rs) vetoes the
        // merge. Measured 2026-07-10: negation (0.015) and budget (0.046)
        // pairs fall UNDER every sane threshold — only the veto stops them;
        // the ideological pair (0.107) is stopped by the threshold itself.
        let sig = |t: &str| crate::stance::direction_signature(t);
        let protected = |dist: f64, x: &str, y: &str| {
            dist > MODEL_DEFAULT_THRESHOLD || crate::stance::directions_conflict(&sig(x), &sig(y))
        };

        assert!(
            protected(
                ideological,
                "Quero mais saúde para o brasileiro vendendo o SUS para empresas competentes",
                "Precisamos subir os salários dos radiologistas que operam as máquinas de raio-x nos postos de saúde",
            ),
            "ideological pair unprotected ({ideological:.3})"
        );
        assert!(
            protected(
                negation,
                "Privatizar a gestão dos postos de saúde do SUS",
                "Proibir a privatização dos postos de saúde do SUS",
            ),
            "negation pair unprotected ({negation:.3}) — consensus signal poisoned"
        );
        assert!(
            protected(
                budget,
                "Reduzir o orçamento da saúde pública para cortar impostos",
                "Aumentar o orçamento da saúde pública mesmo que os impostos subam",
            ),
            "budget pair unprotected ({budget:.3}) — consensus signal poisoned"
        );
        // Control: merges by distance AND the guard must not veto it.
        assert!(
            control < MODEL_DEFAULT_THRESHOLD,
            "control paraphrase should merge: {control:.3} >= {MODEL_DEFAULT_THRESHOLD}"
        );
        assert!(
            !crate::stance::directions_conflict(
                &sig("Contratar mais médicos para os postos de saúde"),
                &sig(
                    "Precisamos de mais profissionais de medicina nas unidades de saúde do bairro"
                ),
            ),
            "stance guard must not veto the control paraphrase"
        );
    }
}

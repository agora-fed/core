//! Pair judge: reads BOTH texts together and decides what the embedding
//! cannot — whether they actually mean the same thing.
//!
//! Motivation (measured 2026-07-10): a 384-dim sentence embedder compresses
//! each text INDEPENDENTLY, so surface overlap dominates context — "A grande
//! Portuguese near-homophone pairs measure
//! cosine 0.068 (UNDER the merge threshold), and the two senses of "banco"
//! measure 0.078, while a legitimate paraphrase can measure 0.116. No
//! per-text representation fixes that; the pair must be read JOINTLY with
//! cross-attention. That is exactly what an NLI (natural language inference)
//! cross-encoder does: premise + hypothesis in one pass → entailment /
//! neutral / contradiction.
//!
//! Reference model: `MoritzLaurer/mDeBERTa-v3-base-xnli-multilingual-nli-2mil7`
//! (DebertaV2ForSequenceClassification, multilingual incl. pt-BR), run locally
//! via candle (pure Rust, CPU — PLAN.md principle 11).
//!
//! Role in the pipeline: the embedding RETRIEVES a merge candidate cheaply;
//! this judge (plus the stance lexicon, `stance.rs`) DISPOSES. It only ever
//! runs on pairs already under the distance threshold, so its per-pair cost
//! (~hundreds of ms on CPU) is paid rarely.

use std::path::Path;
use std::sync::Mutex;

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::debertav2::{Config, DebertaV2SeqClassificationModel};
use tokenizers::{Tokenizer, TruncationParams};

/// Joint reading of an (premise, hypothesis) pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    /// The premise supports the hypothesis — same-ask territory.
    Entailment,
    /// Unrelated or merely same-topic — NOT the same ask.
    Neutral,
    /// The pair asserts opposite things — antagonistic.
    Contradiction,
}

/// Max joint tokens (premise + hypothesis). XNLI models train at 128–256;
/// civic proposals are short, this is a safety net.
const MAX_TOKENS: usize = 256;

/// A local NLI cross-encoder. Same concurrency posture as `ModelEmbedder`:
/// interior mutex, one instance per process, low call volume.
pub struct NliJudge {
    inner: Mutex<Inner>,
}

struct Inner {
    model: DebertaV2SeqClassificationModel,
    tokenizer: Tokenizer,
    device: Device,
}

impl std::fmt::Debug for NliJudge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NliJudge").finish_non_exhaustive()
    }
}

impl NliJudge {
    /// Load `config.json` + `tokenizer.json` + `model.safetensors` from `dir`.
    /// Expects the XNLI label order `{0: entailment, 1: neutral, 2: contradiction}`
    /// (asserted against the config so a differently-trained head fails load,
    /// not inference).
    ///
    /// # Errors
    /// Human-readable message on missing/corrupt artifacts or label mismatch.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let config_raw = std::fs::read_to_string(dir.join("config.json"))
            .map_err(|e| format!("config.json: {e}"))?;
        let config: Config =
            serde_json::from_str(&config_raw).map_err(|e| format!("config.json parse: {e}"))?;
        // Label-order guard: the whole decision inverts if 0 is not entailment.
        let labels: serde_json::Value =
            serde_json::from_str(&config_raw).map_err(|e| format!("config.json labels: {e}"))?;
        let id2label = &labels["id2label"];
        for (idx, expected) in [
            ("0", "entailment"),
            ("1", "neutral"),
            ("2", "contradiction"),
        ] {
            if id2label[idx] != *expected {
                return Err(format!(
                    "unexpected id2label ({id2label}); this judge assumes 0=entailment, 1=neutral, 2=contradiction"
                ));
            }
        }

        let mut tokenizer = Tokenizer::from_file(dir.join("tokenizer.json"))
            .map_err(|e| format!("tokenizer.json: {e}"))?;
        let _ = tokenizer.with_truncation(Some(TruncationParams {
            max_length: MAX_TOKENS,
            ..Default::default()
        }));

        let device = Device::Cpu;
        let weights = std::fs::read(dir.join("model.safetensors"))
            .map_err(|e| format!("model.safetensors: {e}"))?;
        // Buffered (safe) loader; weights ship in f16, compute in f32 on CPU.
        let vb = VarBuilder::from_buffered_safetensors(weights, DType::F32, &device)
            .map_err(|e| format!("safetensors load: {e}"))?;
        // HF checkpoints prefix the backbone tensors with `deberta.`; candle's
        // loader expects the VarBuilder already positioned there (the pooler
        // and classifier heads escape via `vb.root()` internally).
        let model = DebertaV2SeqClassificationModel::load(vb.pp("deberta"), &config, None)
            .map_err(|e| format!("deberta load: {e}"))?;

        tracing::info!(dir = %dir.display(), "consensus NLI judge loaded (pair reading ON)");
        Ok(Self {
            inner: Mutex::new(Inner {
                model,
                tokenizer,
                device,
            }),
        })
    }

    /// Read the pair jointly. Returns the argmax relation plus the
    /// `[entailment, neutral, contradiction]` probabilities (softmax) so the
    /// caller — and the audit log — can see confidence, not just the verdict.
    ///
    /// # Errors
    /// Human-readable message on tokenizer/inference failure; the caller
    /// decides the failure policy (the service treats an erroring judge as
    /// "no opinion" and falls back to distance + stance guard).
    pub fn relation(
        &self,
        premise: &str,
        hypothesis: &str,
    ) -> Result<(Relation, [f32; 3]), String> {
        let guard = match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let encoding = guard
            .tokenizer
            .encode((premise, hypothesis), true)
            .map_err(|e| format!("tokenize pair: {e}"))?;
        let ids = Tensor::new(encoding.get_ids(), &guard.device)
            .and_then(|t| t.unsqueeze(0))
            .map_err(|e| format!("ids tensor: {e}"))?;
        let type_ids = Tensor::new(encoding.get_type_ids(), &guard.device)
            .and_then(|t| t.unsqueeze(0))
            .map_err(|e| format!("type_ids tensor: {e}"))?;
        let mask = Tensor::new(encoding.get_attention_mask(), &guard.device)
            .and_then(|t| t.unsqueeze(0))
            .map_err(|e| format!("mask tensor: {e}"))?;

        let logits = guard
            .model
            .forward(&ids, Some(type_ids), Some(mask))
            .map_err(|e| format!("nli forward: {e}"))?;
        let probs = candle_nn::ops::softmax(&logits, 1)
            .and_then(|t| t.squeeze(0))
            .and_then(|t| t.to_vec1::<f32>())
            .map_err(|e| format!("softmax: {e}"))?;
        let probs: [f32; 3] = [probs[0], probs[1], probs[2]];

        let argmax = probs
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, _)| i)
            .unwrap_or(1);
        let relation = match argmax {
            0 => Relation::Entailment,
            2 => Relation::Contradiction,
            _ => Relation::Neutral,
        };
        Ok((relation, probs))
    }

    /// The merge-gate question, both directions: are these THE SAME ASK?
    ///
    /// Policy anchored to the measured pt-BR matrix (calibration test):
    /// - CONFIDENT contradiction (p ≥ 0.5) in either direction → `false`
    ///   (measured antagonists score 0.84–1.00; a 0.44-plurality fluke on a
    ///   related-but-distinct pair must not read as antagonism);
    /// - CONFIDENT entailment (p ≥ 0.5) in at least one direction → `true`
    ///   (paraphrases entail asymmetrically when one side adds detail —
    ///   measured 0.98 on the creche pair);
    /// - anything else (neutral, weak pluralities) → `false`: same topic at
    ///   best, not the same ask. This is what kills homonyms ("obra do mestre
    ///   Picasso" vs "site foreman"), same-place-different-intervention
    ///   pairs, and related-but-different-scope asks.
    ///
    /// # Errors
    /// Propagates the first inference error (caller decides the fallback).
    pub fn same_ask(&self, a: &str, b: &str) -> Result<bool, String> {
        const CONFIDENT: f32 = 0.5;
        let (_, p_ab) = self.relation(a, b)?;
        if p_ab[2] >= CONFIDENT {
            return Ok(false);
        }
        let (_, p_ba) = self.relation(b, a)?;
        if p_ba[2] >= CONFIDENT {
            return Ok(false);
        }
        let verdict = p_ab[0] >= CONFIDENT || p_ba[0] >= CONFIDENT;
        tracing::debug!(?p_ab, ?p_ba, verdict, "nli same_ask");
        Ok(verdict)
    }
}

impl crate::domain::PairJudge for NliJudge {
    fn same_ask(&self, a: &str, b: &str) -> Result<bool, String> {
        NliJudge::same_ask(self, a, b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Calibration matrix — runs only where CONSENSUS_NLI_DIR points at the
    /// model artifacts (dev machine); CI skips. Prints every judgment so the
    /// merge policy in `same_ask` stays anchored to measured data.
    #[test]
    fn nli_judge_separates_meaning_not_just_topic() {
        let Ok(dir) = std::env::var("CONSENSUS_NLI_DIR") else {
            eprintln!("CONSENSUS_NLI_DIR unset; skipping NLI calibration");
            return;
        };
        let judge = NliJudge::load(Path::new(&dir)).expect("load nli model");
        let show = |name: &str, a: &str, b: &str| {
            let started = std::time::Instant::now();
            let (r_ab, p_ab) = judge.relation(a, b).expect("relation ab");
            let (r_ba, p_ba) = judge.relation(b, a).expect("relation ba");
            let same = judge.same_ask(a, b).expect("same_ask");
            eprintln!(
                "{name}: a→b {r_ab:?} [e={:.2} n={:.2} c={:.2}] | b→a {r_ba:?} [e={:.2} n={:.2} c={:.2}] | same_ask={same} ({}ms)",
                p_ab[0], p_ab[1], p_ab[2], p_ba[0], p_ba[1], p_ba[2],
                started.elapsed().as_millis()
            );
            same
        };

        // Full matrix FIRST (calibration data survives any failing gate).
        let cases: &[(&str, &str, &str, bool)] = &[
            // MUST merge: true paraphrases.
            (
                "paraphrase-creche",
                "Precisamos de uma creche no bairro para as crianças",
                "Faltam vagas em berçário e creche para os bebês da região",
                true,
            ),
            (
                "paraphrase-onibus",
                "Mais ônibus no horário de pico",
                "Aumentar a quantidade de ônibus nos horários de pico",
                true,
            ),
            // Related but NOT the same ask — and the judge is right where the
            // embedding-only calibration was lenient: "horário de pico" ≠
            // "pela manhã" (evening peak exists) and more buses ≠ higher
            // frequency. Strict pair-reading keeps them separate.
            (
                "related-not-same-scope",
                "Mais ônibus no horário de pico",
                "Aumentar a frequência do transporte público pela manhã",
                false,
            ),
            // MUST NOT: same words, different meaning (Picasso class —
            // embedding distance 0.068, UNDER the merge threshold).
            (
                "homonym-picasso",
                "A grande obra do mestre Picasso",
                "A pica de aço do mestre de obras",
                false,
            ),
            (
                "homonym-banco",
                "O banco da praça está quebrado",
                "O banco cobrou juros abusivos na praça de pagamentos",
                false,
            ),
            // MUST NOT: antagonistic (embedding 0.015 / 0.046).
            (
                "negation-privatizar",
                "Privatizar a gestão dos postos de saúde do SUS",
                "Proibir a privatização dos postos de saúde do SUS",
                false,
            ),
            (
                "budget-directions",
                "Reduzir o orçamento da saúde pública para cortar impostos",
                "Aumentar o orçamento da saúde pública mesmo que os impostos subam",
                false,
            ),
            // MUST NOT: same place, different intervention (embedding 0.078).
            (
                "same-place-different-work",
                "Construir uma ciclovia na avenida Brasil",
                "Recapear o asfalto da avenida Brasil",
                false,
            ),
        ];
        let mut wrong: Vec<&str> = Vec::new();
        for (name, a, b, expected) in cases {
            if show(name, a, b) != *expected {
                wrong.push(name);
            }
        }
        assert!(wrong.is_empty(), "misjudged pairs: {wrong:?}");
    }
}

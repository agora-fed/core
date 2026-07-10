//! Pure clustering domain: the embedder port, a deterministic stub embedder, vector math, and
//! the threshold decision. No `sqlx`, no `axum` — everything here is unit-testable in isolation,
//! which is where this crate earns its coverage (TESTING.md).

use dsoc_core::ids::{ClusterId, ProposalId};

/// Embedding dimensionality. Matches the `vector(384)` columns in migration 0130 and a small
/// sentence-embedding model.
pub const EMBEDDING_DIM: usize = 384;

/// Default cosine-distance threshold below which a proposal is considered a near-duplicate of an
/// existing cluster centroid. `<=>` returns cosine distance in `[0, 2]`; identical direction is `0`.
/// Tuned conservatively: only very close proposals merge, so distinct civic asks stay distinct.
pub const DEFAULT_THRESHOLD: f64 = 0.15;

/// Produces a fixed-length embedding for a piece of text.
///
/// Abstracted so the production path can later swap in a local model without touching clustering
/// logic, while tests use a deterministic stub (never a live model — TESTING.md).
pub trait Embedder: Send + Sync {
    /// Embed `text` into an [`EMBEDDING_DIM`]-length vector.
    fn embed(&self, text: &str) -> Vec<f32>;
}

/// Reads a candidate pair JOINTLY and answers "is this the same ask?" —
/// the judgment a per-text embedding cannot make (measured: homonyms like
/// "obra do mestre Picasso" vs "mestre de obras" land UNDER the merge
/// threshold; antagonists land at 0.015). Optional in the cluster service:
/// absent judge = distance + stance lexicon only.
pub trait PairJudge: Send + Sync {
    /// `true` when the two texts assert the same demand.
    ///
    /// # Errors
    /// Implementations surface inference failures as a human-readable message;
    /// the service treats an erroring judge as "no opinion" (fail-open to the
    /// cheaper guards, never wedge the loop).
    fn same_ask(&self, a: &str, b: &str) -> Result<bool, String>;
}

/// A deterministic, dependency-free embedder: hashed bag-of-words (feature hashing) into
/// [`EMBEDDING_DIM`] buckets, then L2-normalised. Same text always yields the same vector, and
/// texts sharing tokens land close together — enough to exercise clustering deterministically.
///
/// This is a stand-in for a local embedding model (PLAN.md open question: "Embedding model choice
/// for `consensus` (local)"). It is intentionally not a semantic model.
#[derive(Debug, Clone, Copy, Default)]
pub struct StubEmbedder;

impl Embedder for StubEmbedder {
    fn embed(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0.0f32; EMBEDDING_DIM];
        for token in tokenize(text) {
            let h = fnv1a_64(token.as_bytes());
            let idx = (h % EMBEDDING_DIM as u64) as usize;
            // Signed feature hashing reduces collisions cancelling out information.
            let sign = if (h >> 63) & 1 == 0 { 1.0 } else { -1.0 };
            v[idx] += sign;
        }
        l2_normalize(&mut v);
        v
    }
}

/// Lowercase, split on non-alphanumeric, drop empties. Deterministic and allocation-light.
fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// 64-bit FNV-1a — a stable hash (unlike `DefaultHasher`, whose output is not guaranteed across
/// releases). Determinism is a hard requirement: clustering must be reproducible and auditable.
fn fnv1a_64(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// Scale a vector to unit L2 norm in place. A zero vector (e.g. empty text) is left untouched.
fn l2_normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// Cosine distance between two equal-length vectors, matching pgvector's `<=>` (`1 - cosine sim`).
/// Returns `1.0` (maximally distant) when either vector has zero magnitude. Used in unit tests to
/// pin threshold behaviour; the database computes the production value with the same definition.
#[must_use]
pub fn cosine_distance(a: &[f32], b: &[f32]) -> f64 {
    debug_assert_eq!(a.len(), b.len());
    let mut dot = 0.0f64;
    let mut na = 0.0f64;
    let mut nb = 0.0f64;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += f64::from(*x) * f64::from(*y);
        na += f64::from(*x) * f64::from(*x);
        nb += f64::from(*y) * f64::from(*y);
    }
    if na == 0.0 || nb == 0.0 {
        return 1.0;
    }
    1.0 - dot / (na.sqrt() * nb.sqrt())
}

/// The outcome of placing a freshly embedded proposal: either it merged into an existing cluster or
/// it seeded a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// The proposal joined `cluster`; the database recomputes the centroid and size.
    Merged {
        /// The cluster the proposal merged into.
        cluster: ClusterId,
        /// The new live size of that cluster.
        size: u32,
    },
    /// The proposal seeded a new single-member cluster.
    Formed {
        /// The newly created cluster.
        cluster: ClusterId,
        /// The proposal that seeded it.
        proposal: ProposalId,
    },
}

/// The nearest existing cluster to a candidate embedding, if any clusters exist for the org.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Nearest {
    /// The closest cluster's id.
    pub cluster: ClusterId,
    /// Its cosine distance to the candidate embedding.
    pub distance: f64,
}

/// The threshold policy result: merge into an existing cluster, or form a new one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Decision {
    /// Merge into the given cluster at the given distance.
    Merge {
        /// Target cluster.
        cluster: ClusterId,
        /// Distance at merge time (stored on the membership edge).
        distance: f64,
    },
    /// Form a new cluster.
    Form,
}

impl Decision {
    /// Apply the threshold policy to the nearest-cluster lookup result.
    ///
    /// Pure policy: merge iff a nearest cluster exists and is within (`<=`) `threshold`. The
    /// boundary is inclusive so an exactly-at-threshold proposal merges — pinned by unit tests.
    #[must_use]
    pub fn decide(nearest: Option<Nearest>, threshold: f64) -> Self {
        match nearest {
            Some(n) if n.distance <= threshold => Decision::Merge {
                cluster: n.cluster,
                distance: n.distance,
            },
            _ => Decision::Form,
        }
    }
}

/// Reject empty / whitespace-only proposal text before embedding (validate at the boundary).
///
/// # Errors
/// Returns [`dsoc_core::Error::Validation`] when `text` has no usable content.
pub fn validate_text(text: &str) -> dsoc_core::Result<()> {
    if text.trim().is_empty() {
        return Err(dsoc_core::Error::Validation(
            "proposal text must not be empty".to_string(),
        ));
    }
    Ok(())
}

/// Format an embedding as the pgvector text literal `[a,b,c]`. We bind this as `text` and cast
/// `::text::vector` in SQL, because the `sqlx` compile-time macros do not map the `vector` type for
/// bind parameters. Deterministic and lossless for `f32`.
#[must_use]
pub fn to_pgvector_literal(embedding: &[f32]) -> String {
    let mut out = String::with_capacity(embedding.len() * 8 + 2);
    out.push('[');
    for (i, x) in embedding.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&x.to_string());
    }
    out.push(']');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_embedder_is_deterministic() {
        let e = StubEmbedder;
        assert_eq!(
            e.embed("orçamento participativo"),
            e.embed("orçamento participativo")
        );
    }

    #[test]
    fn embeddings_have_fixed_dimension() {
        let v = StubEmbedder.embed("creche no bairro");
        assert_eq!(v.len(), EMBEDDING_DIM);
    }

    #[test]
    fn embeddings_are_unit_norm_for_nonempty_text() {
        let v = StubEmbedder.embed("iluminação pública na praça");
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "norm was {norm}");
    }

    #[test]
    fn empty_text_yields_zero_vector() {
        let v = StubEmbedder.embed("");
        assert!(v.iter().all(|x| *x == 0.0));
    }

    #[test]
    fn near_identical_texts_are_close() {
        let e = StubEmbedder;
        let a = e.embed("queremos mais creches no bairro centro");
        let b = e.embed("queremos mais creches no bairro centro agora");
        assert!(
            cosine_distance(&a, &b) < DEFAULT_THRESHOLD,
            "should be near-duplicates"
        );
    }

    #[test]
    fn dissimilar_texts_are_far() {
        let e = StubEmbedder;
        let a = e.embed("creche no bairro centro");
        let b = e.embed("reforma da iluminação avenida litoral");
        assert!(
            cosine_distance(&a, &b) > DEFAULT_THRESHOLD,
            "should be distinct"
        );
    }

    #[test]
    fn identical_vectors_have_zero_distance() {
        let v = StubEmbedder.embed("transparência orçamentária");
        assert!(cosine_distance(&v, &v).abs() < 1e-9);
    }

    #[test]
    fn zero_vector_distance_is_maximal() {
        let zero = vec![0.0f32; EMBEDDING_DIM];
        let v = StubEmbedder.embed("qualquer coisa");
        assert_eq!(cosine_distance(&zero, &v), 1.0);
        assert_eq!(cosine_distance(&v, &zero), 1.0);
    }

    #[test]
    fn tokenize_splits_and_lowercases() {
        let toks = tokenize("Olá, Mundo-2! ");
        assert_eq!(toks, vec!["olá", "mundo", "2"]);
    }

    #[test]
    fn tokenize_empty_is_empty() {
        assert!(tokenize("   ,.! ").is_empty());
    }

    #[test]
    fn fnv1a_is_stable_and_distinct() {
        // Pinned value guards against accidental algorithm drift (determinism is a contract).
        assert_eq!(fnv1a_64(b""), 0xcbf2_9ce4_8422_2325);
        assert_ne!(fnv1a_64(b"creche"), fnv1a_64(b"escola"));
    }

    #[test]
    fn decision_merges_within_threshold() {
        let cluster = ClusterId::new();
        let d = Decision::decide(
            Some(Nearest {
                cluster,
                distance: 0.10,
            }),
            DEFAULT_THRESHOLD,
        );
        assert_eq!(
            d,
            Decision::Merge {
                cluster,
                distance: 0.10
            }
        );
    }

    #[test]
    fn decision_merges_exactly_at_threshold() {
        let cluster = ClusterId::new();
        let d = Decision::decide(
            Some(Nearest {
                cluster,
                distance: DEFAULT_THRESHOLD,
            }),
            DEFAULT_THRESHOLD,
        );
        assert_eq!(
            d,
            Decision::Merge {
                cluster,
                distance: DEFAULT_THRESHOLD
            }
        );
    }

    #[test]
    fn decision_forms_just_past_threshold() {
        let cluster = ClusterId::new();
        let d = Decision::decide(
            Some(Nearest {
                cluster,
                distance: DEFAULT_THRESHOLD + 1e-6,
            }),
            DEFAULT_THRESHOLD,
        );
        assert_eq!(d, Decision::Form);
    }

    #[test]
    fn decision_forms_when_no_cluster() {
        assert_eq!(Decision::decide(None, DEFAULT_THRESHOLD), Decision::Form);
    }

    #[test]
    fn validate_text_rejects_blank() {
        assert!(validate_text("   ").is_err());
        assert!(validate_text("").is_err());
    }

    #[test]
    fn validate_text_accepts_content() {
        assert!(validate_text("proposta válida").is_ok());
    }

    #[test]
    fn pgvector_literal_formats_brackets_and_commas() {
        assert_eq!(to_pgvector_literal(&[1.0, 0.0, -2.5]), "[1,0,-2.5]");
        assert_eq!(to_pgvector_literal(&[]), "[]");
    }

    #[test]
    fn placement_variants_are_distinct() {
        let cluster = ClusterId::new();
        let proposal = ProposalId::new();
        let merged = Placement::Merged { cluster, size: 3 };
        let formed = Placement::Formed { cluster, proposal };
        assert_ne!(merged, formed);
    }
}

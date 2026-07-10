//! Policy-direction guard: keeps IDEOLOGICALLY ANTAGONISTIC asks out of the
//! same cluster even when the embedding says "same topic".
//!
//! Measured motivation (2026-07-10, e5-small on pt-BR civic pairs — see the
//! calibration tests in `model_embedder`): "privatizar os postos do SUS" vs
//! "proibir a privatização dos postos do SUS" sit at cosine distance 0.015 —
//! far *below* true paraphrases (0.072–0.089). No distance threshold can
//! separate stance; sentence embedders encode topic, not policy direction.
//! Merging them would poison the consensus signal: the SLA would fire for a
//! self-contradictory demand the officeholder could rightly dismiss.
//!
//! Approach: a deliberately SMALL, precision-first, auditable lexicon of
//! policy-direction axes (scale up/down, public/private, permit/forbid,
//! open/close, hire/fire) plus negators that flip the axis they precede.
//! Each text compiles to a *direction signature*; two signatures conflict
//! when they take opposite signs on the same axis, or when one negates a
//! content stem the other asserts. The guard only runs on pairs the embedding
//! already placed under the merge threshold, so topic overlap is a given —
//! the lexicon never has to establish "same subject", only "opposite verb".
//!
//! Everything here is pure and deterministic (auditable: the same inputs
//! always veto or not — TESTING.md), and the false-VETO direction is cheap:
//! a wrongly-split cluster keeps both demands alive separately, while a
//! wrong merge corrupts both.

/// One entry of a direction signature. Serialised as short strings so the
/// signature can live in a `text[]` column owned by this crate:
/// `a:<axis><sign>` (axis stance), `n:<stem>` (negated stem), `s:<stem>`
/// (asserted content stem).
///
/// Axes (sign `+` / `-`):
/// - `scale`  — expand vs shrink (aumentar / reduzir)
/// - `public` — toward the public sector vs toward the private one
/// - `permit` — allow vs forbid
/// - `open`   — build/open vs close/tear down
/// - `staff`  — hire vs dismiss
const AXIS_LEXICON: &[(&str, &str, char)] = &[
    // stem prefix (accent-folded, lowercase), axis, sign
    ("aument", "scale", '+'),
    ("amplia", "scale", '+'),
    ("eleva", "scale", '+'),
    ("expand", "scale", '+'),
    ("dobr", "scale", '+'),
    ("reduz", "scale", '-'),
    ("cort", "scale", '-'),
    ("diminu", "scale", '-'),
    ("encolh", "scale", '-'),
    ("estatiz", "public", '+'),
    ("reestatiz", "public", '+'),
    ("nacionaliz", "public", '+'),
    ("privatiz", "public", '-'),
    ("terceiriz", "public", '-'),
    ("vend", "public", '-'),
    ("legaliz", "permit", '+'),
    ("autoriz", "permit", '+'),
    ("permit", "permit", '+'),
    ("proib", "permit", '-'),
    ("criminaliz", "permit", '-'),
    ("constru", "open", '+'),
    ("inaugur", "open", '+'),
    ("reabr", "open", '+'),
    ("fech", "open", '-'),
    ("demol", "open", '-'),
    ("desativ", "open", '-'),
    ("extingu", "open", '-'),
    ("contrat", "staff", '+'),
    ("demit", "staff", '-'),
    ("exoner", "staff", '-'),
];

/// Tokens that INVERT the axis stem they precede (window of
/// [`NEGATOR_WINDOW`] content tokens): "proibir a privatização" asserts
/// `public+`, not `public-`. A negator with no axis stem in reach negates the
/// nearest content stem instead (`n:<stem>`).
const NEGATORS: &[&str] = &["nao", "proib", "imped", "contra", "fim", "acab", "barr"];

/// How many content tokens ahead a negator reaches.
const NEGATOR_WINDOW: usize = 3;

/// Content stems shorter than this carry too little meaning to negate.
const MIN_STEM: usize = 5;

/// Stems are truncated to this length — crude but deterministic stemming
/// ("privatizar" / "privatização" / "privatizem" → `privatiz`).
const STEM_LEN: usize = 8;

/// Compile `text` into its direction signature. Deterministic; safe on any
/// input (unknown words simply contribute `s:` stems).
#[must_use]
pub fn direction_signature(text: &str) -> Vec<String> {
    let tokens: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(fold)
        .collect();

    let mut out: Vec<String> = Vec::new();
    let mut flipped: Vec<usize> = Vec::new(); // token indexes already consumed by a negator

    for (i, tok) in tokens.iter().enumerate() {
        if !is_negator(tok) {
            continue;
        }
        // A negator reaches the next few CONTENT tokens (skip stopword-length ones).
        let mut seen = 0usize;
        for (j, cand) in tokens.iter().enumerate().skip(i + 1) {
            if cand.len() < MIN_STEM {
                continue;
            }
            seen += 1;
            if seen > NEGATOR_WINDOW {
                break;
            }
            if let Some((axis, sign)) = axis_of(cand) {
                out.push(format!("a:{axis}{}", flip(sign)));
                flipped.push(j);
                break;
            }
            // First content stem in reach, no axis: negated assertion.
            out.push(format!("n:{}", stem(cand)));
            flipped.push(j);
            break;
        }
    }

    for (i, tok) in tokens.iter().enumerate() {
        if tok.len() < MIN_STEM || flipped.contains(&i) {
            continue;
        }
        if let Some((axis, sign)) = axis_of(tok) {
            // A negator token that doubles as an axis stem ("proibir") only
            // asserts its own axis when it did NOT flip something ahead.
            out.push(format!("a:{axis}{sign}"));
        }
        out.push(format!("s:{}", stem(tok)));
    }
    out.sort();
    out.dedup();
    out
}

/// Do two signatures take OPPOSITE policy directions?
///
/// True when (a) they hold opposite signs on the same axis — and the sign is
/// unambiguous on each side (a text asserting both directions of an axis,
/// e.g. "aumentar ônibus e reduzir tarifa", conflicts with nothing on that
/// axis) — or (b) one side negates a content stem the other asserts.
#[must_use]
pub fn directions_conflict(a: &[String], b: &[String]) -> bool {
    for axis in ["scale", "public", "permit", "open", "staff"] {
        let (a_plus, a_minus) = axis_signs(a, axis);
        let (b_plus, b_minus) = axis_signs(b, axis);
        let a_only_plus = a_plus && !a_minus;
        let a_only_minus = a_minus && !a_plus;
        let b_only_plus = b_plus && !b_minus;
        let b_only_minus = b_minus && !b_plus;
        if (a_only_plus && b_only_minus) || (a_only_minus && b_only_plus) {
            return true;
        }
    }
    // Negated stem on one side, asserted (and not also negated) on the other.
    let negated_vs_asserted = |x: &[String], y: &[String]| {
        x.iter().filter_map(|e| e.strip_prefix("n:")).any(|n| {
            y.iter().any(|e| e.strip_prefix("s:") == Some(n))
                && !y.iter().any(|e| e.strip_prefix("n:") == Some(n))
        })
    };
    negated_vs_asserted(a, b) || negated_vs_asserted(b, a)
}

fn axis_signs(sig: &[String], axis: &str) -> (bool, bool) {
    let plus = sig.iter().any(|e| e == &format!("a:{axis}+"));
    let minus = sig.iter().any(|e| e == &format!("a:{axis}-"));
    (plus, minus)
}

fn axis_of(token: &str) -> Option<(&'static str, char)> {
    AXIS_LEXICON
        .iter()
        .find(|(prefix, _, _)| token.starts_with(prefix))
        .map(|(_, axis, sign)| (*axis, *sign))
}

fn is_negator(token: &str) -> bool {
    NEGATORS.iter().any(|n| {
        if n.len() <= 4 {
            token == *n
        } else {
            token.starts_with(n)
        }
    })
}

fn flip(sign: char) -> char {
    if sign == '+' {
        '-'
    } else {
        '+'
    }
}

fn stem(token: &str) -> String {
    token.chars().take(STEM_LEN).collect()
}

/// Lowercase + strip pt-BR diacritics, so "privatização" and "privatizacao"
/// stem identically.
fn fold(token: &str) -> String {
    token
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' => 'a',
            'é' | 'ê' => 'e',
            'í' => 'i',
            'ó' | 'ô' | 'õ' => 'o',
            'ú' | 'ü' => 'u',
            'ç' => 'c',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conflict(a: &str, b: &str) -> bool {
        directions_conflict(&direction_signature(a), &direction_signature(b))
    }

    #[test]
    fn direct_negation_conflicts() {
        // Measured at cosine distance 0.015 — indistinguishable by embedding.
        assert!(conflict(
            "Privatizar a gestão dos postos de saúde do SUS",
            "Proibir a privatização dos postos de saúde do SUS",
        ));
    }

    #[test]
    fn opposite_budget_directions_conflict() {
        // Measured at 0.046 — closer than legitimate paraphrases.
        assert!(conflict(
            "Reduzir o orçamento da saúde pública para cortar impostos",
            "Aumentar o orçamento da saúde pública mesmo que os impostos subam",
        ));
    }

    #[test]
    fn sell_vs_strengthen_public_is_layered_defense_territory() {
        // The motivating pair: both "fight for health", ideologically opposed
        // ("vender o SUS" vs "aumentar salários dos radiologistas do SUS").
        // The lexicon sees different axes (public- vs scale+) — no LEXICAL
        // conflict, and that is by design: inferring that a salary raise for
        // public workers means "strengthen the public system" is semantics,
        // not lexicon. This pair is handled by the OTHER layer: it measures
        // cosine 0.107 > threshold 0.10, so the embedding never proposes the
        // merge (asserted end-to-end in model_embedder's calibration test).
        // Documented here so nobody "fixes" the lexicon into guessing.
        assert!(!conflict(
            "Quero mais saúde para o brasileiro vendendo o SUS para empresas competentes",
            "Aumentar os salários dos radiologistas que operam as máquinas de raio-x nos postos de saúde",
        ));
        // But make the same pair EXPLICIT about the public/private axis and
        // the lexicon does catch it, regardless of embedding distance:
        assert!(conflict(
            "Quero mais saúde privatizando a gestão do SUS",
            "Quero mais saúde estatizando os laboratórios que atendem o SUS",
        ));
    }

    #[test]
    fn nao_flips_the_following_axis() {
        assert!(conflict(
            "Não construir o viaduto na praça",
            "Construir o viaduto na praça",
        ));
    }

    #[test]
    fn true_paraphrases_do_not_conflict() {
        assert!(!conflict(
            "Contratar mais médicos para os postos de saúde",
            "Precisamos de mais profissionais de medicina nas unidades de saúde do bairro",
        ));
        assert!(!conflict(
            "Precisamos de uma creche no bairro para as crianças",
            "Faltam vagas em berçário e creche para os bebês da região",
        ));
    }

    #[test]
    fn same_side_negations_agree() {
        // Both AGAINST: no conflict between two anti-privatisation asks.
        assert!(!conflict(
            "Proibir a privatização do SUS",
            "Não privatizar o SUS",
        ));
    }

    #[test]
    fn both_directions_in_one_text_is_ambiguous_not_conflicting() {
        // "aumentar X e reduzir Y" holds both scale signs — it must not veto
        // against a text that only increases.
        assert!(!conflict(
            "Aumentar a frota de ônibus e reduzir a tarifa",
            "Aumentar a frequência dos ônibus no horário de pico",
        ));
    }

    #[test]
    fn unrelated_texts_do_not_conflict() {
        assert!(!conflict(
            "Recapear o asfalto da avenida principal",
            "Mais policiamento e iluminação na praça central",
        ));
    }
}

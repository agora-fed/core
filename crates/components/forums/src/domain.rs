//! Pure forum domain: topic validation, territorial templates and the
//! **threshold policy** — no sqlx, no axum (TESTING.md: cheap coverage lives here).

use dsoc_core::{Error, Result};
use uuid::Uuid;

/// Maximum path depth (`sp/santos/saude` = 3 segments).
pub const MAX_DEPTH: usize = 3;
/// Cap of targets (offices) per directed topic (B1) — mirrors the multi-recipient
/// proposal limit (0537): directing means addressing ONE cohesive group, not
/// institutional spam.
pub const MAX_TOPIC_TARGETS: usize = 10;
/// Defensive title limit.
pub const MAX_TITLE_LEN: usize = 200;
/// Limite defensivo do corpo.
pub const MAX_BODY_LEN: usize = 40_000;
/// Defensive comment limit.
pub const MAX_COMMENT_LEN: usize = 10_000;

/// A default section of a territorial forum (state/municipality), materialized on demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerritorialSection {
    /// Segmento de caminho (`saude`).
    pub slug: &'static str,
    /// Display name (`Saúde`).
    pub name: &'static str,
}

/// The 7 default sections — identical for state and municipality, only the two
/// institucionais: estado = Assembleia Legislativa + Governo do Estado;
/// municipality = City Council + Mayor's Office (decision of plan v3).
#[must_use]
pub fn territorial_sections(esfera_municipal: bool) -> [TerritorialSection; 7] {
    let (leg, gov) = if esfera_municipal {
        (
            TerritorialSection {
                slug: "camara-municipal",
                name: "Câmara Municipal",
            },
            TerritorialSection {
                slug: "prefeitura",
                name: "Prefeitura",
            },
        )
    } else {
        (
            TerritorialSection {
                slug: "assembleia",
                name: "Assembleia Legislativa",
            },
            TerritorialSection {
                slug: "governo",
                name: "Governo do Estado",
            },
        )
    };
    [
        leg,
        gov,
        TerritorialSection {
            slug: "saude",
            name: "Saúde",
        },
        TerritorialSection {
            slug: "educacao",
            name: "Educação",
        },
        TerritorialSection {
            slug: "seguranca",
            name: "Segurança",
        },
        TerritorialSection {
            slug: "infraestrutura",
            name: "Infraestrutura",
        },
        TerritorialSection {
            slug: "lazer",
            name: "Lazer",
        },
    ]
}

/// Validate a `/f/...` path: 1..=MAX_DEPTH segments of `[a-z0-9-]`, none empty.
///
/// # Errors
/// [`Error::Validation`] for a malformed path.
pub fn validate_path(path: &str) -> Result<Vec<&str>> {
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    if segments.is_empty() || segments.len() > MAX_DEPTH {
        return Err(Error::Validation(format!(
            "caminho de fórum deve ter 1 a {MAX_DEPTH} segmentos"
        )));
    }
    for s in &segments {
        if s.is_empty()
            || s.len() > 63
            || !s
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            || s.starts_with('-')
        {
            return Err(Error::Validation(format!(
                "segmento inválido no caminho: {s:?}"
            )));
        }
    }
    Ok(segments)
}

/// A validated topic (title/body within limits; creation requires a verified citizen —
/// the identity document is already mandatory and validated at signup, so the gate is Email level).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTopic {
    /// Topic title.
    pub title: String,
    /// Topic body.
    pub body: String,
}

impl NewTopic {
    /// Validate and normalize title/body.
    ///
    /// # Errors
    /// [`Error::Validation`] for an empty title/body or one above the limit.
    pub fn validate(title: &str, body: &str) -> Result<Self> {
        let title = title.trim();
        let body = body.trim();
        if title.is_empty() || title.chars().count() > MAX_TITLE_LEN {
            return Err(Error::Validation(format!(
                "título deve ter 1 a {MAX_TITLE_LEN} caracteres"
            )));
        }
        if body.is_empty() || body.chars().count() > MAX_BODY_LEN {
            return Err(Error::Validation(format!(
                "corpo deve ter 1 a {MAX_BODY_LEN} caracteres"
            )));
        }
        Ok(Self {
            title: title.to_owned(),
            body: body.to_owned(),
        })
    }
}

/// A citizen's stance on a topic (0544; the neutral option was removed in ADR-0019): in favour
/// or against — one per citizen, mutable. Feeds the points scoreboard (ADR-0019).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stance {
    /// In favour of the topic's proposal.
    Favor,
    /// Contra.
    Contra,
}

impl Stance {
    /// The stable text stored in `forum_topic_vote.stance` (matches the CHECK).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Favor => "favor",
            Self::Contra => "contra",
        }
    }

    /// Parse a stance coming from outside (a request).
    ///
    /// # Errors
    /// [`Error::Validation`] for a value outside the closed set.
    pub fn parse_input(s: &str) -> Result<Self> {
        match s.trim() {
            "favor" => Ok(Self::Favor),
            "contra" => Ok(Self::Contra),
            other => Err(Error::Validation(format!(
                "posição deve ser favor ou contra: {other}"
            ))),
        }
    }
}

/// Validate a comment.
///
/// # Errors
/// [`Error::Validation`] for an empty body or one above the limit.
pub fn validate_comment(body: &str) -> Result<String> {
    let body = body.trim();
    if body.is_empty() || body.chars().count() > MAX_COMMENT_LEN {
        return Err(Error::Validation(format!(
            "comentário deve ter 1 a {MAX_COMMENT_LEN} caracteres"
        )));
    }
    Ok(body.to_owned())
}

/// Normalize the target list (mandate_ids) of a directed topic (B1): preserves
/// the ORDER of first occurrence, drops duplicates and truncates at
/// [`MAX_TOPIC_TARGETS`]. Pure and total — validating that the mandates EXIST
/// belongs to the service layer (it needs I/O). An empty list = topic with no
/// target (dispatches to the section's curated contact).
#[must_use]
pub fn sanitize_targets(ids: &[Uuid]) -> Vec<Uuid> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for &id in ids {
        if out.len() >= MAX_TOPIC_TARGETS {
            break;
        }
        if seen.insert(id) {
            out.push(id);
        }
    }
    out
}

/// **Threshold policy** (the heart): given the total of COUNTABLE interactions,
/// the forum's threshold list and the index of the next threshold not yet fired,
/// returns the thresholds that MUST fire now (one dispatch per threshold, in order)
/// and the new index. Federated interactions never enter `interactions`.
///
/// Pure and total: replays/duplicates never re-fire (the index only moves forward).
#[must_use]
pub fn thresholds_to_fire(
    interactions: i64,
    thresholds: &[i32],
    next_idx: usize,
) -> (Vec<i32>, usize) {
    let mut fire = Vec::new();
    let mut idx = next_idx;
    while idx < thresholds.len() && i64::from(thresholds[idx]) <= interactions {
        fire.push(thresholds[idx]);
        idx += 1;
    }
    (fire, idx)
}

/// **Bridging claim** (D8.2 of the critique plan — Polis/vTaiwan-style synthesis):
/// measures how much an argument is endorsed ACROSS the topic's for×against
/// divide. The input is the number of ENDORSEMENTS of the argument (a `favor`
/// vote on the comment) coming from each side of the topic:
/// - `favor_side` = endorsers whose stance ON THE TOPIC is `favor`;
/// - `contra_side` = endorsers whose stance ON THE TOPIC is `contra`.
///
/// Returns the **harmonic mean** of the two sides:
///
/// ```text
/// bridge = 2·f·c / (f + c)   (0 whenever either side is 0)
/// ```
///
/// Why the harmonic mean (and not the for+against sum of the cheering scoreboard):
/// - it is **0** when only one side endorses — that is cheering, not a bridge;
/// - it is **dominated by the weaker side** (the "weak link"): a bridge is worth what
///   the side that supports it LEAST gives it, so recruiting only your own faction
///   does not raise the score — you must convince the other side;
/// - it grows with volume AND with balance (maximal when `f = c`), rewarding the
///   argument that UNITES those who disagree, not the one that shouts loudest.
///
/// Pure and total: no I/O, trivially testable (the formula lives here, not in SQL).
#[must_use]
#[allow(clippy::cast_precision_loss)] // contagens de votos são pequenas — sem perda relevante
pub fn bridge_score(favor_side: i64, contra_side: i64) -> f64 {
    if favor_side <= 0 || contra_side <= 0 {
        return 0.0;
    }
    let f = favor_side as f64;
    let c = contra_side as f64;
    2.0 * f * c / (f + c)
}

/// An argument is a **bridge** when it is endorsed by BOTH sides of the topic
/// (`favor_side ≥ 1` AND `contra_side ≥ 1`). Only bridges enter the consensus section.
#[must_use]
pub const fn is_bridge(favor_side: i64, contra_side: i64) -> bool {
    favor_side > 0 && contra_side > 0
}

/// Slugify a municipality/entity name into a path segment
/// (`"São Paulo"` → `"sao-paulo"`). Same rule as the SQL seed.
#[must_use]
pub fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_dash = true; // suprime '-' inicial
    for c in name.chars() {
        let mapped: Option<char> = match c.to_lowercase().next().unwrap_or(c) {
            'á' | 'à' | 'â' | 'ã' | 'ä' => Some('a'),
            'é' | 'è' | 'ê' | 'ë' => Some('e'),
            'í' | 'ì' | 'î' | 'ï' => Some('i'),
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => Some('o'),
            'ú' | 'ù' | 'û' | 'ü' => Some('u'),
            'ç' => Some('c'),
            'ñ' => Some('n'),
            lc if lc.is_ascii_lowercase() || lc.is_ascii_digit() => Some(lc),
            _ => None,
        };
        match mapped {
            Some(ch) => {
                out.push(ch);
                last_dash = false;
            }
            None => {
                if !last_dash {
                    out.push('-');
                    last_dash = true;
                }
            }
        }
    }
    out.trim_end_matches('-').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn territorial_sections_swap_institutional_pair() {
        let uf = territorial_sections(false);
        let mun = territorial_sections(true);
        assert_eq!(uf[0].slug, "assembleia");
        assert_eq!(mun[0].slug, "camara-municipal");
        assert_eq!(uf[1].slug, "governo");
        assert_eq!(mun[1].slug, "prefeitura");
        // The 5 thematic sections are identical.
        assert_eq!(&uf[2..], &mun[2..]);
        assert_eq!(uf.len(), 7);
    }

    #[test]
    fn validate_path_accepts_up_to_three_segments() {
        assert_eq!(validate_path("senado").unwrap(), vec!["senado"]);
        assert_eq!(validate_path("/sp/santos/").unwrap(), vec!["sp", "santos"]);
        assert_eq!(
            validate_path("sp/santos/saude").unwrap(),
            vec!["sp", "santos", "saude"]
        );
        assert!(validate_path("a/b/c/d").is_err());
        assert!(validate_path("Maiusculo").is_err());
        assert!(validate_path("com espaço").is_err());
        assert!(validate_path("-inicio").is_err());
        assert!(validate_path("").is_err());
    }

    #[test]
    fn new_topic_validates_bounds() {
        assert!(NewTopic::validate(" Vacinas ", " corpo ").is_ok());
        assert!(NewTopic::validate("", "b").is_err());
        assert!(NewTopic::validate(&"a".repeat(MAX_TITLE_LEN + 1), "b").is_err());
        assert!(NewTopic::validate("t", "").is_err());
    }

    #[test]
    fn stance_round_trips_and_rejects_unknown() {
        for (s, txt) in [(Stance::Favor, "favor"), (Stance::Contra, "contra")] {
            assert_eq!(s.as_str(), txt);
            assert_eq!(Stance::parse_input(txt).unwrap(), s);
        }
        assert_eq!(Stance::parse_input(" favor ").unwrap(), Stance::Favor);
        // The neutral option was removed (ADR-0019) — it is now an invalid value.
        assert!(Stance::parse_input("ponderacao").is_err());
        assert!(Stance::parse_input("pro").is_err());
        assert!(Stance::parse_input("").is_err());
    }

    #[test]
    fn comment_validates_bounds() {
        assert_eq!(validate_comment(" oi ").unwrap(), "oi");
        assert!(validate_comment("   ").is_err());
        assert!(validate_comment(&"x".repeat(MAX_COMMENT_LEN + 1)).is_err());
    }

    #[test]
    fn sanitize_targets_dedups_preserves_order_and_caps() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let c = Uuid::from_u128(3);
        // Empty → empty (topic with no target).
        assert_eq!(sanitize_targets(&[]), Vec::<Uuid>::new());
        // Dedupe preserving the order of FIRST occurrence.
        assert_eq!(sanitize_targets(&[a, b, a, c, b]), vec![a, b, c]);
        // Truncate at MAX_TOPIC_TARGETS (unique ids).
        let many: Vec<Uuid> = (0..(MAX_TOPIC_TARGETS as u128 + 5))
            .map(Uuid::from_u128)
            .collect();
        let out = sanitize_targets(&many);
        assert_eq!(out.len(), MAX_TOPIC_TARGETS);
        assert_eq!(out[0], Uuid::from_u128(0));
    }

    // --- the threshold policy ---

    #[test]
    fn thresholds_fire_in_order_once_each() {
        let t = [1000, 10_000, 100_000];
        assert_eq!(thresholds_to_fire(999, &t, 0), (vec![], 0));
        assert_eq!(thresholds_to_fire(1000, &t, 0), (vec![1000], 1));
        // Replay with the index already advanced: nothing re-fires.
        assert_eq!(thresholds_to_fire(1500, &t, 1), (vec![], 1));
        // Salto direto por dois patamares: ambos disparam, em ordem.
        assert_eq!(thresholds_to_fire(20_000, &t, 0), (vec![1000, 10_000], 2));
        // Fim da lista.
        assert_eq!(thresholds_to_fire(1_000_000, &t, 3), (vec![], 3));
    }

    #[test]
    fn thresholds_empty_list_never_fires() {
        assert_eq!(thresholds_to_fire(999_999, &[], 0), (vec![], 0));
    }

    // --- bridging claim (D8.2) ---

    #[test]
    fn bridge_score_zero_when_one_side_absent() {
        // Only one side endorses → not a bridge, just cheering.
        assert_eq!(bridge_score(5, 0), 0.0);
        assert_eq!(bridge_score(0, 5), 0.0);
        assert_eq!(bridge_score(0, 0), 0.0);
        // Negative counts (defensive) also collapse to zero.
        assert_eq!(bridge_score(-3, 4), 0.0);
        assert!(!is_bridge(5, 0));
        assert!(!is_bridge(0, 5));
        assert!(is_bridge(1, 1));
    }

    #[test]
    fn bridge_score_is_dominated_by_the_weaker_side() {
        // Weak link: recruiting only your own side barely moves the score.
        // f=10, c=1 → 2·10·1/11 ≈ 1.818 (close to 2·c, not to f).
        let lopsided = bridge_score(10, 1);
        assert!((lopsided - 1.818_181).abs() < 1e-4);
        // Balancing the weak side is worth MUCH more than inflating the strong one:
        // dobrar o forte (10→20) mal muda; dobrar o fraco (1→2) quase dobra.
        assert!(bridge_score(20, 1) < bridge_score(10, 2));
    }

    #[test]
    fn bridge_score_rewards_balance_and_volume() {
        // Maximal at balance: for a fixed sum (f+c=10), 5/5 > 9/1.
        assert!(bridge_score(5, 5) > bridge_score(9, 1));
        // Balanced, grows with volume: 5/5 > 2/2 > 1/1.
        assert!(bridge_score(5, 5) > bridge_score(2, 2));
        assert!(bridge_score(2, 2) > bridge_score(1, 1));
        // When f = c, the harmonic mean is exactly c (weak link = both).
        assert_eq!(bridge_score(4, 4), 4.0);
        assert_eq!(bridge_score(1, 1), 1.0);
    }

    #[test]
    fn bridge_score_is_symmetric() {
        // A bridge has no "favoured side": swapping favor↔contra yields the same score.
        assert_eq!(bridge_score(3, 7), bridge_score(7, 3));
        assert_eq!(bridge_score(2, 9), bridge_score(9, 2));
    }

    #[test]
    fn slugify_handles_accents_and_spaces() {
        assert_eq!(slugify("São Paulo"), "sao-paulo");
        assert_eq!(slugify("SANTA BÁRBARA D'OESTE"), "santa-barbara-d-oeste");
        assert_eq!(slugify("Comissão de Ética"), "comissao-de-etica");
        assert_eq!(slugify("---"), "");
    }
}

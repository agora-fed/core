//! POSITIVE-VALUE layer of the scorecard (Block C of the product plan): from the same
//! `answered`/`ignored` counters and the median latency the ledger already projects, we derive the
//! mandate's *showcase* — a responsiveness badge/tier, the "answers in ~N days", the streak of
//! consecutive answers and the peer comparison (percentile).
//!
//! Everything here is PURE and deterministic (no `sqlx`, no clock, no I/O): the same input always
//! produces the same badge, so it is 100% testable at the unit layer (TESTING.md) and reproducible
//! from the ledger. The queries feeding these functions live in the gateway (runtime sqlx); this
//! layer only decides, never reads.
//!
//! Golden rule of the plan: every NEGATIVE consequence the platform amplifies (the silence) has
//! its POSITIVE counterpart. These functions are the raw material of that positive version.

use crate::domain::Outcome;

/// Minimum rate (%) for the Gold badge. Below it the mandate answers well, but not "almost always".
const GOLD_MIN_RATE: u32 = 80;
/// Maximum median latency (hours) for Gold: answering a lot, but slowly, is not Gold. 72h = 3d.
const GOLD_MAX_MEDIAN_HOURS: f64 = 72.0;
/// Minimum rate (%) for the Silver badge.
const SILVER_MIN_RATE: u32 = 60;
/// Minimum rate (%) for the Bronze badge.
const BRONZE_MIN_RATE: u32 = 30;

/// Public responsiveness badge of the mandate — the "medal" that gives the official a POSITIVE
/// reason to claim and use the scorecard. Ascending order of merit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponsivenessTier {
    /// No demand recorded yet — nothing to judge (not a bad grade, an absence of data).
    Unrated,
    /// Starting out: answers a minority of demands. Neutral framing, never accusatory.
    Building,
    /// Bronze: answers a meaningful share of demands.
    Bronze,
    /// Silver: answers most demands.
    Silver,
    /// Gold: answers nearly all of them — and fast.
    Gold,
}

impl ResponsivenessTier {
    /// Stable (English) token for serialization/CSS. Never changes without a contract migration.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            ResponsivenessTier::Unrated => "unrated",
            ResponsivenessTier::Building => "building",
            ResponsivenessTier::Bronze => "bronze",
            ResponsivenessTier::Silver => "silver",
            ResponsivenessTier::Gold => "gold",
        }
    }

    /// Short pt-BR label for the badge.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            ResponsivenessTier::Unrated => "Sem dados ainda",
            ResponsivenessTier::Building => "Em construção",
            ResponsivenessTier::Bronze => "Bronze",
            ResponsivenessTier::Silver => "Prata",
            ResponsivenessTier::Gold => "Ouro",
        }
    }

    /// pt-BR showcase sentence explaining the badge in one line.
    #[must_use]
    pub const fn blurb(self) -> &'static str {
        match self {
            ResponsivenessTier::Unrated => "Nenhuma demanda cidadã registrada até agora.",
            ResponsivenessTier::Building => "Está começando a responder às demandas cidadãs.",
            ResponsivenessTier::Bronze => "Responde a uma parcela relevante das demandas.",
            ResponsivenessTier::Silver => "Responde à maioria das demandas cidadãs.",
            ResponsivenessTier::Gold => "Responde à quase totalidade das demandas — e rápido.",
        }
    }

    /// Medal emoji for visual reinforcement (feed, card, badge).
    #[must_use]
    pub const fn medal(self) -> &'static str {
        match self {
            ResponsivenessTier::Unrated => "•",
            ResponsivenessTier::Building => "🌱",
            ResponsivenessTier::Bronze => "🥉",
            ResponsivenessTier::Silver => "🥈",
            ResponsivenessTier::Gold => "🥇",
        }
    }
}

/// Response rate as an integer 0–100 (answered / total), or `None` when there are no demands.
/// Mirrors `responseRate` on the front end (web/src/lib/format.ts) to keep both sides identical.
#[must_use]
pub fn response_rate_pct(answered: i64, ignored: i64) -> Option<u32> {
    let total = answered + ignored;
    if total <= 0 {
        return None;
    }
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let pct = ((answered.max(0) as f64 / total as f64) * 100.0).round() as u32;
    Some(pct.min(100))
}

/// Derive the badge from the counters and the median latency (hours). Gold requires BOTH:
/// a high rate AND a fast answer; answering a lot but slowly stops at Silver.
#[must_use]
pub fn responsiveness_tier(
    answered: i64,
    ignored: i64,
    median_hours: Option<f64>,
) -> ResponsivenessTier {
    let Some(rate) = response_rate_pct(answered, ignored) else {
        return ResponsivenessTier::Unrated;
    };
    let is_fast = median_hours.is_none_or(|h| h <= GOLD_MAX_MEDIAN_HOURS);
    if rate >= GOLD_MIN_RATE && is_fast {
        ResponsivenessTier::Gold
    } else if rate >= SILVER_MIN_RATE {
        ResponsivenessTier::Silver
    } else if rate >= BRONZE_MIN_RATE {
        ResponsivenessTier::Bronze
    } else {
        ResponsivenessTier::Building
    }
}

/// "Answers in ~N days" from the median latency: hours → days, rounded to 1 decimal. `None`
/// when the mandate has answered nothing yet.
#[must_use]
pub fn responds_in_days(median_hours: Option<f64>) -> Option<f64> {
    median_hours.map(|h| (h / 24.0 * 10.0).round() / 10.0)
}

/// Streak of answers: how many of the MOST RECENT demands were answered back to back, with no
/// silence in between. `outcomes` must arrive newest-first. A high streak is a consistency medal
/// ("🔥 5 answers in a row").
#[must_use]
pub fn current_answer_streak(outcomes: &[Outcome]) -> u32 {
    let mut streak = 0u32;
    for outcome in outcomes {
        if outcome.is_answered() {
            streak += 1;
        } else {
            break;
        }
    }
    streak
}

/// Percentage of peers this mandate BEATS (strictly higher rate). The basis of "better than
/// X% of peers" / "Top Y%". `peer_rates` are the rates (0–100) of comparable peers, EXCLUDING
/// this mandate itself. `None` when there are no peers to compare against.
#[must_use]
pub fn better_than_pct(your_rate: u32, peer_rates: &[u32]) -> Option<u32> {
    if peer_rates.is_empty() {
        return None;
    }
    let worse = peer_rates.iter().filter(|&&r| r < your_rate).count();
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let pct = ((worse as f64 / peer_rates.len() as f64) * 100.0).round() as u32;
    Some(pct.min(100))
}

/// "Top Y%": the complement of [`better_than_pct`] — the smaller, the closer to the top. `better_than = 90%`
/// becomes `Top 10%`. Minimum 1% (never "Top 0%").
#[must_use]
pub fn top_pct(better_than: u32) -> u32 {
    (100u32.saturating_sub(better_than)).max(1)
}

/// Mean of a peer set's rates (0–100), or `None` when the set is empty. Feeds the
/// "você respondeu 78% · média do RS 21%".
#[must_use]
pub fn average_rate(peer_rates: &[u32]) -> Option<u32> {
    if peer_rates.is_empty() {
        return None;
    }
    let sum: u64 = peer_rates.iter().map(|&r| u64::from(r)).sum();
    #[allow(clippy::cast_possible_truncation)]
    Some((sum / peer_rates.len() as u64) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_is_none_without_demands_and_rounds_otherwise() {
        assert_eq!(response_rate_pct(0, 0), None);
        assert_eq!(response_rate_pct(1, 1), Some(50));
        assert_eq!(response_rate_pct(7, 3), Some(70));
        // 2/3 = 66.66… rounds to 67.
        assert_eq!(response_rate_pct(2, 1), Some(67));
        assert_eq!(response_rate_pct(10, 0), Some(100));
    }

    #[test]
    fn tier_is_unrated_without_any_demand() {
        assert_eq!(responsiveness_tier(0, 0, None), ResponsivenessTier::Unrated);
    }

    #[test]
    fn gold_requires_high_rate_and_speed() {
        // 90% and fast (24h) → Gold.
        assert_eq!(
            responsiveness_tier(9, 1, Some(24.0)),
            ResponsivenessTier::Gold
        );
        // 90% but slow (200h > 72h) → drops to Silver, not Gold.
        assert_eq!(
            responsiveness_tier(9, 1, Some(200.0)),
            ResponsivenessTier::Silver
        );
        // 90% with no known latency (no measurement) → treated as fast → Gold.
        assert_eq!(responsiveness_tier(9, 1, None), ResponsivenessTier::Gold);
    }

    #[test]
    fn silver_bronze_building_thresholds() {
        assert_eq!(
            responsiveness_tier(6, 4, Some(10.0)),
            ResponsivenessTier::Silver
        ); // 60%
        assert_eq!(
            responsiveness_tier(3, 7, Some(10.0)),
            ResponsivenessTier::Bronze
        ); // 30%
        assert_eq!(
            responsiveness_tier(2, 8, Some(10.0)),
            ResponsivenessTier::Building
        ); // 20%
    }

    #[test]
    fn tier_tokens_are_stable() {
        for (t, key, label) in [
            (ResponsivenessTier::Unrated, "unrated", "Sem dados ainda"),
            (ResponsivenessTier::Building, "building", "Em construção"),
            (ResponsivenessTier::Bronze, "bronze", "Bronze"),
            (ResponsivenessTier::Silver, "silver", "Prata"),
            (ResponsivenessTier::Gold, "gold", "Ouro"),
        ] {
            assert_eq!(t.key(), key);
            assert_eq!(t.label(), label);
            assert!(!t.blurb().is_empty());
            assert!(!t.medal().is_empty());
        }
    }

    #[test]
    fn responds_in_days_converts_and_rounds() {
        assert_eq!(responds_in_days(None), None);
        assert_eq!(responds_in_days(Some(24.0)), Some(1.0));
        assert_eq!(responds_in_days(Some(36.0)), Some(1.5));
        assert_eq!(responds_in_days(Some(4.0)), Some(0.2)); // 0.1666… → 0.2
    }

    #[test]
    fn streak_counts_leading_answers_only() {
        use Outcome::{Answered, Ignored};
        // newest first: 3 answers, then a silence → streak 3.
        assert_eq!(
            current_answer_streak(&[Answered, Answered, Answered, Ignored, Answered]),
            3
        );
        // starts with a silence → streak 0.
        assert_eq!(current_answer_streak(&[Ignored, Answered]), 0);
        assert_eq!(current_answer_streak(&[]), 0);
        assert_eq!(current_answer_streak(&[Answered, Answered]), 2);
    }

    #[test]
    fn peer_comparison_percentiles() {
        // you 78%, peers [10,20,30,90] → beats 3 of 4 = 75%.
        assert_eq!(better_than_pct(78, &[10, 20, 30, 90]), Some(75));
        assert_eq!(top_pct(75), 25);
        // no peers → None.
        assert_eq!(better_than_pct(78, &[]), None);
        // mean of the peers.
        assert_eq!(average_rate(&[10, 20, 30]), Some(20));
        assert_eq!(average_rate(&[]), None);
    }

    #[test]
    fn top_pct_never_zero() {
        // beats 100% of peers → Top 1% (never Top 0%).
        assert_eq!(top_pct(100), 1);
        assert_eq!(top_pct(0), 100);
    }
}

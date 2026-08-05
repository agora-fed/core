//! # Electorate-proportional threshold (statistical legitimacy).
//!
//! PURE, shared maths of the consequence trigger. Both the gateway (PROPOSAL
//! threshold, item 4 / 0.30.1) and the forums (office-dispatch threshold,
//! ADR-0019 / D3 of the critique plan) use the SAME yardstick: the RELATIVE
//! effort to summon an office is identical in Roraima and in Sao Paulo.
//! Absolute volume is gameable; a fraction of the electorate is not.
//!
//! Formula:
//!
//! ```text
//! threshold = clamp( ceil(fraction × electorate), floor, ceiling )
//! ```
//!
//! - `fraction`  — share of the territory's electorate (e.g. 0.0005 = 0.05%).
//! - `floor`     — absolute minimum (never fires below it).
//! - `ceiling`   — absolute maximum (avoids impossible targets in SP/national).
//! - unknown electorate (`None`) → falls back to the floor; NEVER blocks
//!   anything (fail-safe: with no territory data, require the minimum).
//!
//! Why it lives in `dsoc-core`: it is a domain primitive — no I/O, testable,
//! and it must be identical on both paths (proposals and forums) — real DRY,
//! not speculative (two concrete consumers today).

/// Compute the electorate-proportional threshold.
///
/// `voters = None` (a territory with no electorate row) returns the floor —
/// the trigger still exists at its minimum, never switched off by missing data.
///
/// A `fraction` outside `(0, 1]`, or `floor > ceil`, are the caller's
/// responsibility (config is validated at the edge); here we only apply the
/// arithmetic and the `clamp`, which is robust even with floor ≥ ceil (it
/// returns the ceiling).
#[must_use]
pub fn proportional_threshold(voters: Option<i64>, fraction: f64, floor: i64, ceil: i64) -> i64 {
    let Some(voters) = voters else {
        return floor;
    };
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    let raw = (voters.max(0) as f64 * fraction).ceil() as i64;
    // `clamp` panics when floor > ceil; normalize defensively.
    raw.clamp(floor.min(ceil), ceil)
}

/// Privacy graduated by territory size (D5/D6 of the plan).
///
/// In small municipalities, exposing WHO supported or took a stance on a local
/// topic is, in practice, a retaliation map (real identity document + small
/// territory + a scoreboard that takes sides). This check says whether the
/// territory is small enough that ONLY the aggregate may be public — the
/// individual stance attribution stays protected (pseudonymized/omitted).
///
/// `true` when the electorate is KNOWN and smaller than `min_electorate`.
/// A territory with no data (`None`) → `false`: we never presume protection
/// without a basis (and a large/national territory is never small). Applying
/// this ONLY to municipalities (never to small UFs, which do not exist) is the
/// caller's decision — it passes `voters` from the municipal scope only.
#[must_use]
pub fn is_small_electorate(voters: Option<i64>, min_electorate: i64) -> bool {
    matches!(voters, Some(v) if v < min_electorate)
}

#[cfg(test)]
mod tests {
    use super::{is_small_electorate, proportional_threshold};

    #[test]
    fn escala_com_o_eleitorado_e_faz_clamp() {
        // 0.05% of 1M voters = 500.
        assert_eq!(
            proportional_threshold(Some(1_000_000), 0.0005, 25, 10_000),
            500
        );
        // A small municipality falls back to the floor (0.05% of 8k = 4 → floor 25).
        assert_eq!(proportional_threshold(Some(8_000), 0.0005, 25, 10_000), 25);
        // National hits the ceiling (0.05% of 155M = 77,500 → ceiling 10,000).
        assert_eq!(
            proportional_threshold(Some(155_000_000), 0.0005, 25, 10_000),
            10_000
        );
        // No territory data: floor, never blocks.
        assert_eq!(proportional_threshold(None, 0.0005, 25, 10_000), 25);
        // Rounds up: 0.05% of 50,001 = 25.0005 → 26.
        assert_eq!(proportional_threshold(Some(50_001), 0.0005, 25, 10_000), 26);
    }

    #[test]
    fn piso_dez_dos_foruns_preserva_o_comportamento_do_adr_0019() {
        // Forums use floor 10 (the former fixed ESCALATION_POINTS of ADR-0019).
        // Small municipality / territory without data → still requires 10.
        assert_eq!(proportional_threshold(None, 0.0005, 10, 10_000), 10);
        assert_eq!(proportional_threshold(Some(5_000), 0.0005, 10, 10_000), 10);
        // Mid-sized city (300k voters): 0.05% = 150 — proportional, above the floor.
        assert_eq!(
            proportional_threshold(Some(300_000), 0.0005, 10, 10_000),
            150
        );
        // Large capital (~SP, 9M voters): 0.05% = 4,500 — proportional, still under the ceiling.
        assert_eq!(
            proportional_threshold(Some(9_000_000), 0.0005, 10, 10_000),
            4_500
        );
        // Escala nacional (155M eleitores): 0,05% = 77.500 → bate no teto 10.000.
        assert_eq!(
            proportional_threshold(Some(155_000_000), 0.0005, 10, 10_000),
            10_000
        );
    }

    #[test]
    fn piso_maior_que_teto_nao_entra_em_panico() {
        // Invalid config (floor > ceiling) returns the ceiling instead of panicking.
        assert_eq!(proportional_threshold(Some(1_000_000), 0.0005, 100, 50), 50);
    }

    #[test]
    fn eleitorado_negativo_e_tratado_como_zero() {
        // Defensive: corrupted data must never become a negative threshold.
        assert_eq!(proportional_threshold(Some(-10), 0.0005, 10, 10_000), 10);
    }

    #[test]
    fn municipio_pequeno_protege_a_atribuicao_individual() {
        // Below the threshold → aggregate only.
        assert!(is_small_electorate(Some(2_000), 5_000));
        assert!(is_small_electorate(Some(4_999), 5_000));
        // At or above the threshold → individual exposure allowed.
        assert!(!is_small_electorate(Some(5_000), 5_000));
        assert!(!is_small_electorate(Some(500_000), 5_000));
        // No territory data → do not presume protection.
        assert!(!is_small_electorate(None, 5_000));
    }
}

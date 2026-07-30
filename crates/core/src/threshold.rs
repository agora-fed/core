//! # Patamar proporcional ao eleitorado (legitimidade estatística).
//!
//! Matemática PURA e compartilhada do gatilho de consequência. Tanto o gateway
//! (patamar das PROPOSTAS, item 4 / 0.30.1) quanto os fóruns (patamar de
//! encaminhamento ao gabinete, ADR-0019 / D3 do plano de crítica) usam a MESMA
//! régua: o esforço RELATIVO para acionar um gabinete é igual em Roraima e em
//! São Paulo. Volume absoluto é gameável; fração do eleitorado, não.
//!
//! Fórmula:
//!
//! ```text
//! patamar = clamp( ceil(fração × eleitorado), piso, teto )
//! ```
//!
//! - `fração`   — parcela do eleitorado do território (ex.: 0,0005 = 0,05%).
//! - `piso`     — mínimo absoluto (nunca dispara com menos que isso).
//! - `teto`     — máximo absoluto (evita alvos impossíveis em SP/nacional).
//! - eleitorado desconhecido (`None`) → cai no piso; NUNCA bloqueia nada
//!   (fail-safe: sem dado do território, exige o mínimo).
//!
//! Por que fica em `dsoc-core`: é primitivo de domínio, sem I/O, testável, e
//! precisa ser idêntico nos dois caminhos (propostas e fóruns) — DRY real, não
//! especulativo (dois consumidores concretos hoje).

/// Calcula o patamar proporcional ao eleitorado.
///
/// `voters = None` (território sem linha de eleitorado) devolve o piso — o
/// gatilho segue existindo no mínimo, nunca é desligado por falta de dado.
///
/// `fraction` fora de `(0, 1]`, ou `floor > ceil`, são responsabilidade do
/// chamador (a config é validada na borda); aqui só aplicamos a conta e o
/// `clamp`, que é robusto mesmo com piso ≥ teto (devolve o teto).
#[must_use]
pub fn proportional_threshold(voters: Option<i64>, fraction: f64, floor: i64, ceil: i64) -> i64 {
    let Some(voters) = voters else {
        return floor;
    };
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    let raw = (voters.max(0) as f64 * fraction).ceil() as i64;
    // `clamp` entra em pânico se floor > ceil; normalizamos por segurança.
    raw.clamp(floor.min(ceil), ceil)
}

/// Privacidade graduada por tamanho de território (D5/D6 do plano).
///
/// Em municípios pequenos, expor QUEM apoiou/tomou posição num tópico local é,
/// na prática, um mapa de retaliação (CPF real + território pequeno + placar que
/// toma lado). Esta checagem diz se o território é pequeno o suficiente para que
/// SÓ o agregado seja público — a atribuição individual de posição fica
/// protegida (pseudonimizada/omitida).
///
/// `true` quando o eleitorado é CONHECIDO e menor que `min_electorate`.
/// Território sem dado (`None`) → `false`: não presumimos proteção sem base
/// (e território grande/nacional nunca é pequeno). A decisão de aplicar isso
/// APENAS a municípios (e não a UFs pequenas, que não existem) é do chamador,
/// que só passa `voters` do escopo municipal.
#[must_use]
pub fn is_small_electorate(voters: Option<i64>, min_electorate: i64) -> bool {
    matches!(voters, Some(v) if v < min_electorate)
}

#[cfg(test)]
mod tests {
    use super::{is_small_electorate, proportional_threshold};

    #[test]
    fn escala_com_o_eleitorado_e_faz_clamp() {
        // 0,05% de 1M de eleitores = 500.
        assert_eq!(
            proportional_threshold(Some(1_000_000), 0.0005, 25, 10_000),
            500
        );
        // Município pequeno cai no piso (0,05% de 8k = 4 → piso 25).
        assert_eq!(proportional_threshold(Some(8_000), 0.0005, 25, 10_000), 25);
        // Nacional bate no teto (0,05% de 155M = 77.500 → teto 10.000).
        assert_eq!(
            proportional_threshold(Some(155_000_000), 0.0005, 25, 10_000),
            10_000
        );
        // Sem dado do território: piso, nunca bloqueia.
        assert_eq!(proportional_threshold(None, 0.0005, 25, 10_000), 25);
        // Arredonda pra cima: 0,05% de 50.001 = 25,0005 → 26.
        assert_eq!(proportional_threshold(Some(50_001), 0.0005, 25, 10_000), 26);
    }

    #[test]
    fn piso_dez_dos_foruns_preserva_o_comportamento_do_adr_0019() {
        // Fóruns usam piso 10 (o antigo ESCALATION_POINTS fixo do ADR-0019).
        // Município pequeno / território sem dado → segue exigindo 10.
        assert_eq!(proportional_threshold(None, 0.0005, 10, 10_000), 10);
        assert_eq!(proportional_threshold(Some(5_000), 0.0005, 10, 10_000), 10);
        // Cidade média (300k eleitores): 0,05% = 150 — proporcional, acima do piso.
        assert_eq!(
            proportional_threshold(Some(300_000), 0.0005, 10, 10_000),
            150
        );
        // Capital grande (~SP, 9M eleitores): 0,05% = 4.500 — proporcional, ainda sob o teto.
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
        // Config inválida (piso > teto) devolve o teto em vez de estourar.
        assert_eq!(proportional_threshold(Some(1_000_000), 0.0005, 100, 50), 50);
    }

    #[test]
    fn eleitorado_negativo_e_tratado_como_zero() {
        // Defensivo: dado corrompido não vira patamar negativo.
        assert_eq!(proportional_threshold(Some(-10), 0.0005, 10, 10_000), 10);
    }

    #[test]
    fn municipio_pequeno_protege_a_atribuicao_individual() {
        // Abaixo do limiar → só agregado.
        assert!(is_small_electorate(Some(2_000), 5_000));
        assert!(is_small_electorate(Some(4_999), 5_000));
        // No limiar ou acima → exposição individual permitida.
        assert!(!is_small_electorate(Some(5_000), 5_000));
        assert!(!is_small_electorate(Some(500_000), 5_000));
        // Sem dado do território → não presume proteção.
        assert!(!is_small_electorate(None, 5_000));
    }
}

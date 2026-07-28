//! Registro eleitoral brasileiro — **Título de Eleitor** (ADR-0015). A validação algorítmica
//! (algoritmo oficial TSE) foi movida de `dsoc-gateway::titulo_eleitor` para trás do trait
//! agnóstico [`dsoc_core::VoterRegistration`].
//!
//! Estrutura: `SEQ (8 dígitos) | UF (2 dígitos, 01–28) | DV1 | DV2`. DV1 = mod 11 do peso 2..=9
//! sobre SEQ (regra especial pra SP/MG); DV2 = mod 11 sobre UF+DV1 com pesos 7,8,9. A promoção a
//! `verified` (cross-check com dados abertos do TSE) fica pra fatia posterior.

use dsoc_core::{Error, VoterRegistration};

/// Extrai só dígitos + valida comprimento 12.
fn normalize(raw: &str) -> Option<Vec<u8>> {
    let digits: Vec<u8> = raw
        .chars()
        .filter_map(|c| c.to_digit(10).map(|d| d as u8))
        .collect();
    if digits.len() != 12 {
        return None;
    }
    Some(digits)
}

/// Valida os 2 DVs conforme regra oficial TSE (docs.tse.jus.br + Serpro).
/// Resto 10 vira 0; para SP/MG (UF 01/02) um DV 0 vira 1.
fn check_digits(d: &[u8]) -> bool {
    let seq = &d[..8];
    let uf = ((d[8] as u32) * 10 + d[9] as u32) as u8;
    if !(1..=28).contains(&uf) {
        return false;
    }
    let dv1_expected = d[10];
    let dv2_expected = d[11];

    // DV1
    let mut sum: u32 = 0;
    for (i, dig) in seq.iter().enumerate() {
        sum += (*dig as u32) * ((i as u32) + 2);
    }
    let mut dv1 = (sum % 11) as u8;
    if dv1 == 10 {
        dv1 = 0;
    }
    if dv1 == 0 && matches!(uf, 1 | 2) {
        dv1 = 1;
    }
    if dv1 != dv1_expected {
        return false;
    }

    // DV2
    let d8 = d[8] as u32;
    let d9 = d[9] as u32;
    let d10 = dv1 as u32;
    let sum2 = d8 * 7 + d9 * 8 + d10 * 9;
    let mut dv2 = (sum2 % 11) as u8;
    if dv2 == 10 {
        dv2 = 0;
    }
    if dv2 == 0 && matches!(uf, 1 | 2) {
        dv2 = 1;
    }
    dv2 == dv2_expected
}

/// Registro eleitoral brasileiro (Título de Eleitor). Impl de [`VoterRegistration`].
#[derive(Debug, Clone, Copy, Default)]
pub struct BrVoterRegistration;

impl VoterRegistration for BrVoterRegistration {
    fn validate(&self, raw: &str) -> Result<String, Error> {
        // Mensagens públicas idênticas às do gateway atual (não muda a UX da rota).
        let Some(digits) = normalize(raw) else {
            return Err(Error::Validation(
                "O título deve ter 12 dígitos (sem pontos ou espaços).".to_owned(),
            ));
        };
        if !check_digits(&digits) {
            return Err(Error::Validation(
                "Título de eleitor inválido — verifique os dígitos e tente novamente.".to_owned(),
            ));
        }
        Ok(digits.iter().map(|d| char::from(b'0' + d)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_and_rejects_wrong_length() {
        assert!(normalize("123").is_none());
        assert!(normalize("12345678 9012").is_some());
    }

    #[test]
    fn valid_titulo_passes() {
        // SEQ=00000001, UF=03 (RJ): DV1=9, DV2=6.
        let raw = "000000010396";
        assert!(check_digits(&normalize(raw).unwrap()));
    }

    #[test]
    fn invalid_dv_rejected() {
        assert!(!check_digits(&normalize("000000010397").unwrap()));
    }

    #[test]
    fn invalid_uf_rejected() {
        assert!(!check_digits(&normalize("000000019900").unwrap()));
    }

    #[test]
    fn voter_registration_validate_roundtrips() {
        let reg = BrVoterRegistration;
        assert_eq!(reg.validate("0000-0001-0396").unwrap(), "000000010396");
        assert!(reg.validate("123").is_err());
        assert!(reg.validate("000000010397").is_err());
    }
}

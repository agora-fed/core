//! Camada de VALOR POSITIVO do placar (Bloco C do plano de produto): a partir dos mesmos contadores
//! `answered`/`ignored` e da latência mediana que o ledger já projeta, derivamos a *vitrine* do
//! mandato — um selo/tier de responsividade, o "responde em ~N dias", a sequência (streak) de
//! respostas e o comparativo com pares (percentil).
//!
//! Tudo aqui é PURO e determinístico (sem `sqlx`, sem relógio, sem I/O): a mesma entrada sempre
//! produz o mesmo selo, então é 100% testável na camada unitária (TESTING.md) e reproduzível a
//! partir do ledger. As consultas que alimentam estas funções vivem no gateway (runtime sqlx); esta
//! camada só decide, nunca lê.
//!
//! Regra de ouro do plano: toda consequência NEGATIVA que a plataforma amplifica (o silêncio) tem
//! sua versão POSITIVA. Estas funções são a matéria-prima dessa versão positiva.

use crate::domain::Outcome;

/// Taxa mínima (%) para o selo Ouro. Abaixo disso o mandato responde bem, mas não "quase sempre".
const GOLD_MIN_RATE: u32 = 80;
/// Latência mediana máxima (horas) para o Ouro: responder muito, mas devagar, não é Ouro. 72h = 3d.
const GOLD_MAX_MEDIAN_HOURS: f64 = 72.0;
/// Taxa mínima (%) para o selo Prata.
const SILVER_MIN_RATE: u32 = 60;
/// Taxa mínima (%) para o selo Bronze.
const BRONZE_MIN_RATE: u32 = 30;

/// Selo público de responsividade do mandato — a "medalha" que dá ao político um motivo POSITIVO
/// pra reivindicar e usar o placar. Ordem crescente de mérito.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponsivenessTier {
    /// Nenhuma demanda registrada ainda — nada a julgar (não é uma nota ruim, é ausência de dado).
    Unrated,
    /// Começando: responde a uma minoria das demandas. Enquadramento neutro, nunca acusatório.
    Building,
    /// Bronze: responde a uma parcela relevante das demandas.
    Bronze,
    /// Prata: responde à maioria das demandas.
    Silver,
    /// Ouro: responde à quase totalidade — e rápido.
    Gold,
}

impl ResponsivenessTier {
    /// Token estável (inglês) pra serialização/CSS. Nunca muda sem migração de contrato.
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

    /// Rótulo curto pt-BR pro selo.
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

    /// Frase de vitrine pt-BR que explica o selo em uma linha.
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

    /// Emoji da medalha pra reforço visual (feed, card, badge).
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

/// Taxa de resposta como inteiro 0–100 (respondidas / total), ou `None` quando não há demandas.
/// Espelha `responseRate` do front (web/src/lib/format.ts) pra manter os dois lados idênticos.
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

/// Deriva o selo a partir dos contadores e da latência mediana (horas). O Ouro exige as DUAS coisas:
/// alta taxa E resposta rápida; responder muito, porém devagar, para na Prata.
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

/// "Responde em ~N dias" a partir da latência mediana: horas → dias, arredondado a 1 casa. `None`
/// quando o mandato ainda não respondeu nada.
#[must_use]
pub fn responds_in_days(median_hours: Option<f64>) -> Option<f64> {
    median_hours.map(|h| (h / 24.0 * 10.0).round() / 10.0)
}

/// Sequência (streak) de respostas: quantas demandas MAIS RECENTES foram respondidas seguidas, sem
/// nenhum silêncio no meio. `outcomes` deve vir do mais recente pro mais antigo. Um streak alto é
/// uma medalha de consistência ("🔥 5 respostas seguidas").
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

/// Percentual de pares que este mandato SUPERA (taxa estritamente maior). É a base do "melhor que
/// X% dos pares" / "Top Y%". `peer_rates` são as taxas (0–100) dos pares comparáveis, EXCLUINDO o
/// próprio mandato. `None` quando não há pares com quem comparar.
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

/// "Top Y%": complemento de [`better_than_pct`] — quanto menor, mais no topo. `better_than = 90%`
/// vira `Top 10%`. Mínimo 1% (nunca "Top 0%").
#[must_use]
pub fn top_pct(better_than: u32) -> u32 {
    (100u32.saturating_sub(better_than)).max(1)
}

/// Média das taxas de um conjunto de pares (0–100), ou `None` quando o conjunto é vazio. Alimenta o
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
        // 2/3 = 66.66… arredonda pra 67.
        assert_eq!(response_rate_pct(2, 1), Some(67));
        assert_eq!(response_rate_pct(10, 0), Some(100));
    }

    #[test]
    fn tier_is_unrated_without_any_demand() {
        assert_eq!(responsiveness_tier(0, 0, None), ResponsivenessTier::Unrated);
    }

    #[test]
    fn gold_requires_high_rate_and_speed() {
        // 90% e rápido (24h) → Ouro.
        assert_eq!(
            responsiveness_tier(9, 1, Some(24.0)),
            ResponsivenessTier::Gold
        );
        // 90% mas lento (200h > 72h) → cai pra Prata, não Ouro.
        assert_eq!(
            responsiveness_tier(9, 1, Some(200.0)),
            ResponsivenessTier::Silver
        );
        // 90% sem latência conhecida (nenhuma medida) → considerado rápido → Ouro.
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
        // mais recente primeiro: 3 respostas, depois um silêncio → streak 3.
        assert_eq!(
            current_answer_streak(&[Answered, Answered, Answered, Ignored, Answered]),
            3
        );
        // começa com silêncio → streak 0.
        assert_eq!(current_answer_streak(&[Ignored, Answered]), 0);
        assert_eq!(current_answer_streak(&[]), 0);
        assert_eq!(current_answer_streak(&[Answered, Answered]), 2);
    }

    #[test]
    fn peer_comparison_percentiles() {
        // você 78%, pares [10,20,30,90] → supera 3 de 4 = 75%.
        assert_eq!(better_than_pct(78, &[10, 20, 30, 90]), Some(75));
        assert_eq!(top_pct(75), 25);
        // sem pares → None.
        assert_eq!(better_than_pct(78, &[]), None);
        // média dos pares.
        assert_eq!(average_rate(&[10, 20, 30]), Some(20));
        assert_eq!(average_rate(&[]), None);
    }

    #[test]
    fn top_pct_never_zero() {
        // supera 100% dos pares → Top 1% (nunca Top 0%).
        assert_eq!(top_pct(100), 1);
        assert_eq!(top_pct(0), 100);
    }
}

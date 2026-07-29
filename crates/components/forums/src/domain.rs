//! Domínio puro dos fóruns: validação de tópico, templates territoriais e a
//! **política de patamares** — sem sqlx, sem axum (TESTING.md: cobertura barata aqui).

use dsoc_core::{Error, Result};

/// Profundidade máxima do caminho (`sp/santos/saude` = 3 segmentos).
pub const MAX_DEPTH: usize = 3;
/// Limite defensivo do título.
pub const MAX_TITLE_LEN: usize = 200;
/// Limite defensivo do corpo.
pub const MAX_BODY_LEN: usize = 40_000;
/// Limite defensivo do comentário.
pub const MAX_COMMENT_LEN: usize = 10_000;

/// Uma seção padrão de fórum territorial (estado/município), materializada sob demanda.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerritorialSection {
    /// Segmento de caminho (`saude`).
    pub slug: &'static str,
    /// Nome de exibição (`Saúde`).
    pub name: &'static str,
}

/// As 7 seções padrão — iguais para estado e município, mudando só os dois
/// institucionais: estado = Assembleia Legislativa + Governo do Estado;
/// município = Câmara Municipal + Prefeitura (decisão do plano v3).
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

/// Valida um caminho `/f/...`: 1..=MAX_DEPTH segmentos `[a-z0-9-]`, sem vazio.
///
/// # Errors
/// [`Error::Validation`] para caminho malformado.
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

/// Um tópico validado (título/corpo com limites; criação exige cidadão verificado —
/// o CPF já é obrigatório e validado no cadastro, então o gate é o nível Email).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTopic {
    /// Título do tópico.
    pub title: String,
    /// Corpo do tópico.
    pub body: String,
}

impl NewTopic {
    /// Valida e normaliza título/corpo.
    ///
    /// # Errors
    /// [`Error::Validation`] para título/corpo vazio ou acima do limite.
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

/// A posição de um cidadão num tópico (0544; ponderação eliminada em ADR-0019): a favor
/// ou contra — uma por cidadão, mutável. Alimenta o placar por pontos (ADR-0019).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stance {
    /// A favor da proposta do tópico.
    Favor,
    /// Contra.
    Contra,
}

impl Stance {
    /// O texto estável gravado em `forum_topic_vote.stance` (casa com a CHECK).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Favor => "favor",
            Self::Contra => "contra",
        }
    }

    /// Interpreta uma posição vinda de fora (request).
    ///
    /// # Errors
    /// [`Error::Validation`] para valor fora do conjunto fechado.
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

/// Valida um comentário.
///
/// # Errors
/// [`Error::Validation`] para corpo vazio ou acima do limite.
pub fn validate_comment(body: &str) -> Result<String> {
    let body = body.trim();
    if body.is_empty() || body.chars().count() > MAX_COMMENT_LEN {
        return Err(Error::Validation(format!(
            "comentário deve ter 1 a {MAX_COMMENT_LEN} caracteres"
        )));
    }
    Ok(body.to_owned())
}

/// **Política de patamares** (o coração): dado o total de interações CONTÁVEIS,
/// a lista de patamares do fórum e o índice do próximo patamar ainda não disparado,
/// devolve os patamares que DEVEM disparar agora (um envio por patamar, em ordem)
/// e o novo índice. Interações federadas nunca entram em `interactions`.
///
/// Pura e total: replays/duplicatas não re-disparam (o índice só avança).
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

/// Slugifica um nome de município/entidade para segmento de caminho
/// (`"São Paulo"` → `"sao-paulo"`). Mesma regra do seed SQL.
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
        // As 5 temáticas são idênticas.
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
        // Ponderação foi eliminada (ADR-0019) — agora é valor inválido.
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

    // --- a política de patamares ---

    #[test]
    fn thresholds_fire_in_order_once_each() {
        let t = [1000, 10_000, 100_000];
        assert_eq!(thresholds_to_fire(999, &t, 0), (vec![], 0));
        assert_eq!(thresholds_to_fire(1000, &t, 0), (vec![1000], 1));
        // Replay com o índice avançado: nada re-dispara.
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

    #[test]
    fn slugify_handles_accents_and_spaces() {
        assert_eq!(slugify("São Paulo"), "sao-paulo");
        assert_eq!(slugify("SANTA BÁRBARA D'OESTE"), "santa-barbara-d-oeste");
        assert_eq!(slugify("Comissão de Ética"), "comissao-de-etica");
        assert_eq!(slugify("---"), "");
    }
}

//! Verificação de identidade contra a base autorizada de CPFs (SaaS `cpf-verify`), R-KYC #49.
//! Movido de `dsoc-auth::identity_verify` para trás da fronteira `l10n_br` (ADR-0015).
//!
//! Enquanto [`crate::document::CpfVerifier`] só olha o dígito verificador, aqui confrontamos
//! **nome + data de nascimento + sexo + CPF** com a base e recebemos um VEREDITO em faixas
//! (espelha o `scorer.py`: ACEITA/REVISA/ESCALA/REJEITA — não é probabilidade, é faixa
//! calibrada). O serviço devolve só o veredito, nunca o dado armazenado.
//!
//! **Degradação graciosa:** sem `CPF_VERIFY_URL` configurada, ou se o serviço estiver fora do ar,
//! devolvemos [`Faixa::Skipped`] (fail-open) — o cadastro segue com o comportamento atual (CPF só
//! `Validated`), em vez de travar todo mundo por causa de uma dependência externa. Só um veredito
//! `REJEITA` REAL do serviço bloqueia. Trocar pra fail-closed é uma linha, quando fizer sentido.

use std::sync::Arc;

use dsoc_core::IdentityBand;
use serde::Deserialize;

/// Faixa de confiança do confronto (espelha `scorer.py`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Faixa {
    /// Nome e data batem com folga — identidade forte.
    Aceita,
    /// Bate, mas merece revisão humana.
    Revisa,
    /// Sinal ambíguo (ex.: mudança de sobrenome) — escalar.
    Escala,
    /// Não bate — bloquear.
    Rejeita,
    /// Verificação não executada (serviço ausente/fora) — segue como hoje.
    Skipped,
}

impl Faixa {
    fn parse(s: &str) -> Self {
        match s {
            "ACEITA" => Self::Aceita,
            "REVISA" => Self::Revisa,
            "ESCALA" => Self::Escala,
            "REJEITA" => Self::Rejeita,
            _ => Self::Skipped,
        }
    }

    /// Forma estável pra persistência/auditoria.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Aceita => "aceita",
            Self::Revisa => "revisa",
            Self::Escala => "escala",
            Self::Rejeita => "rejeita",
            Self::Skipped => "skipped",
        }
    }
}

impl From<Faixa> for IdentityBand {
    fn from(f: Faixa) -> Self {
        match f {
            Faixa::Aceita => IdentityBand::Accept,
            Faixa::Revisa => IdentityBand::Review,
            Faixa::Escala => IdentityBand::Escalate,
            Faixa::Rejeita => IdentityBand::Reject,
            Faixa::Skipped => IdentityBand::Skipped,
        }
    }
}

/// O que enviamos pra verificar.
#[derive(Debug, Clone)]
pub struct IdentityQuery {
    /// CPF normalizado (11 dígitos, sem pontuação).
    pub cpf: String,
    /// Nome completo informado pelo cidadão.
    pub nome: String,
    /// Data de nascimento `YYYY-MM-DD` (ou `aaaammdd`), se informada.
    pub nascimento: Option<String>,
    /// Sexo `M`/`F`, se informado.
    pub sexo: Option<String>,
}

/// Resposta verdict-only do serviço.
#[derive(Debug, Clone, Deserialize)]
pub struct IdentityVerdict {
    #[serde(default)]
    pub found: bool,
    /// A faixa crua do serviço (`ACEITA`/…); use [`IdentityVerdict::faixa`] pra tipar.
    #[serde(rename = "faixa", default = "faixa_skipped_raw")]
    pub faixa_raw: String,
    #[serde(default)]
    pub score: f32,
    #[serde(default)]
    pub nascimento_confere: Option<bool>,
    #[serde(default)]
    pub sexo_confere: Option<bool>,
    #[serde(default)]
    pub origem: i32,
    #[serde(default)]
    pub motivos: Vec<String>,
}

fn faixa_skipped_raw() -> String {
    "SKIPPED".to_owned()
}

impl IdentityVerdict {
    /// Veredito de "não verificado" — usado na degradação graciosa.
    #[must_use]
    pub fn skipped() -> Self {
        Self {
            found: false,
            faixa_raw: faixa_skipped_raw(),
            score: 0.0,
            nascimento_confere: None,
            sexo_confere: None,
            origem: 0,
            motivos: vec!["verificação não executada".to_owned()],
        }
    }

    /// A faixa tipada.
    #[must_use]
    pub fn faixa(&self) -> Faixa {
        Faixa::parse(&self.faixa_raw)
    }

    /// Cadastro pode prosseguir? Tudo menos `REJEITA` segue (Skipped inclusive — fail-open).
    #[must_use]
    pub fn allows_registration(&self) -> bool {
        !matches!(self.faixa(), Faixa::Rejeita)
    }

    /// Identidade fortemente confirmada (ACEITA) → candidata a `CpfStatus::Verified`.
    #[must_use]
    pub fn is_strong(&self) -> bool {
        matches!(self.faixa(), Faixa::Aceita)
    }

    /// Precisa de revisão humana (REVISA/ESCALA).
    #[must_use]
    pub fn needs_review(&self) -> bool {
        matches!(self.faixa(), Faixa::Revisa | Faixa::Escala)
    }
}

/// Porta plugável de verificação de identidade (nível SaaS BR).
#[async_trait::async_trait]
pub trait IdentityVerifier: Send + Sync {
    /// Confronta a query com a base. Nunca entra em pânico; erro de transporte vira `skipped`.
    async fn verify_identity(&self, q: &IdentityQuery) -> IdentityVerdict;
}

/// Sem serviço configurado: sempre `skipped` (cadastro segue como hoje).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopIdentityVerifier;

#[async_trait::async_trait]
impl IdentityVerifier for NoopIdentityVerifier {
    async fn verify_identity(&self, _q: &IdentityQuery) -> IdentityVerdict {
        IdentityVerdict::skipped()
    }
}

/// Cliente HTTP pro SaaS `cpf-verify` (ClusterIP interno; Bearer key). Fail-open: qualquer erro
/// de transporte/parse vira `skipped` e loga um aviso — o serviço fora do ar não trava cadastro.
pub struct HttpIdentityVerifier {
    endpoint: String,
    api_key: String,
    client: reqwest::Client,
}

impl std::fmt::Debug for HttpIdentityVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Nunca vaza a api_key no Debug.
        f.debug_struct("HttpIdentityVerifier")
            .field("endpoint", &self.endpoint)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl HttpIdentityVerifier {
    /// Constrói a partir de `CPF_VERIFY_URL` (base, ex. `http://cpf-verify.cpf-verify.svc:8090`)
    /// e `CPF_VERIFY_API_KEY`. Retorna `None` se qualquer uma faltar (→ usar Noop).
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let base = std::env::var("CPF_VERIFY_URL")
            .ok()
            .filter(|s| !s.is_empty())?;
        let api_key = std::env::var("CPF_VERIFY_API_KEY")
            .ok()
            .filter(|s| !s.is_empty())?;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .ok()?;
        Some(Self {
            endpoint: format!("{}/v1/verify/identity", base.trim_end_matches('/')),
            api_key,
            client,
        })
    }
}

#[async_trait::async_trait]
impl IdentityVerifier for HttpIdentityVerifier {
    async fn verify_identity(&self, q: &IdentityQuery) -> IdentityVerdict {
        let body = serde_json::json!({
            "cpf": q.cpf,
            "nome": q.nome,
            "nascimento": q.nascimento,
            "sexo": q.sexo,
        });
        let resp = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await;
        match resp {
            Ok(r) if r.status().is_success() => match r.json::<IdentityVerdict>().await {
                Ok(v) => v,
                Err(err) => {
                    tracing::warn!(error = %err, "cpf-verify: resposta ilegível; degradando");
                    IdentityVerdict::skipped()
                }
            },
            Ok(r) => {
                tracing::warn!(status = %r.status(), "cpf-verify: HTTP não-2xx; degradando");
                IdentityVerdict::skipped()
            }
            Err(err) => {
                tracing::warn!(error = %err, "cpf-verify: transporte falhou; degradando");
                IdentityVerdict::skipped()
            }
        }
    }
}

/// Fábrica: HTTP se o env estiver configurado, senão Noop (degradação graciosa).
#[must_use]
pub fn from_env() -> Arc<dyn IdentityVerifier> {
    match HttpIdentityVerifier::from_env() {
        Some(v) => {
            tracing::info!("cpf-verify: verificação de identidade ATIVA");
            Arc::new(v)
        }
        None => {
            tracing::info!("cpf-verify: sem CPF_VERIFY_URL — verificação de identidade em modo Noop");
            Arc::new(NoopIdentityVerifier)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verdict(faixa: &str, found: bool) -> IdentityVerdict {
        IdentityVerdict {
            found,
            faixa_raw: faixa.to_owned(),
            score: 0.9,
            nascimento_confere: Some(true),
            sexo_confere: Some(true),
            origem: 0,
            motivos: vec![],
        }
    }

    #[test]
    fn faixa_parsing_e_decisoes() {
        assert_eq!(verdict("ACEITA", true).faixa(), Faixa::Aceita);
        assert!(verdict("ACEITA", true).is_strong());
        assert!(verdict("ACEITA", true).allows_registration());

        assert!(verdict("REVISA", true).needs_review());
        assert!(verdict("REVISA", true).allows_registration());
        assert!(!verdict("REVISA", true).is_strong());

        assert!(verdict("ESCALA", true).needs_review());
        assert!(verdict("ESCALA", true).allows_registration());

        assert!(!verdict("REJEITA", true).allows_registration());
        assert!(!verdict("REJEITA", true).is_strong());
    }

    #[test]
    fn skipped_faz_fail_open() {
        let s = IdentityVerdict::skipped();
        assert_eq!(s.faixa(), Faixa::Skipped);
        assert!(s.allows_registration(), "sem serviço, cadastro segue");
        assert!(!s.is_strong());
        assert!(!s.needs_review());
    }

    #[test]
    fn faixa_desconhecida_vira_skipped() {
        assert_eq!(verdict("BANANA", true).faixa(), Faixa::Skipped);
    }

    #[test]
    fn faixa_mapeia_pro_band_agnostico() {
        assert_eq!(IdentityBand::from(Faixa::Aceita), IdentityBand::Accept);
        assert_eq!(IdentityBand::from(Faixa::Rejeita), IdentityBand::Reject);
        assert_eq!(IdentityBand::from(Faixa::Skipped), IdentityBand::Skipped);
    }

    #[test]
    fn deserializa_resposta_do_servico() {
        let json = r#"{"found":true,"faixa":"ACEITA","score":0.98,
            "nascimento_confere":true,"sexo_confere":true,"origem":0,"motivos":["ok"]}"#;
        let v: IdentityVerdict = serde_json::from_str(json).expect("parse");
        assert!(v.found);
        assert_eq!(v.faixa(), Faixa::Aceita);
        assert!(v.is_strong());
    }

    #[tokio::test]
    async fn noop_sempre_skipped() {
        let q = IdentityQuery {
            cpf: "52998224725".to_owned(),
            nome: "Fulano".to_owned(),
            nascimento: None,
            sexo: None,
        };
        let v = NoopIdentityVerifier.verify_identity(&q).await;
        assert_eq!(v.faixa(), Faixa::Skipped);
    }
}

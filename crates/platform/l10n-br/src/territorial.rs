//! Hierarquia territorial brasileira — **IBGE** (ADR-0015). Lê a tabela `municipio_ibge` (migration
//! 0651) por trás do trait agnóstico [`dsoc_core::TerritorialProvider`]. Query em runtime (sem
//! regen do cache sqlx), como o gateway já fazia.

use dsoc_core::{Error, Municipality, Result, TerritorialProvider};
use dsoc_db::Db;

/// Provedor territorial brasileiro. A subdivisão de 1º nível é a **UF** (sigla de 2 letras); a
/// unidade de base é o **município** (código IBGE).
#[derive(Clone)]
pub struct BrTerritorialProvider {
    db: Db,
}

impl std::fmt::Debug for BrTerritorialProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrTerritorialProvider")
            .field("db", &"PgPool")
            .finish()
    }
}

impl BrTerritorialProvider {
    /// Constrói a partir do pool de conexões compartilhado.
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait::async_trait]
impl TerritorialProvider for BrTerritorialProvider {
    async fn municipalities(&self, subdivision: &str) -> Result<Vec<Municipality>> {
        let rows: Vec<(i32, String)> = sqlx::query_as(
            "SELECT codigo_ibge, nome FROM municipio_ibge WHERE uf = $1 ORDER BY nome",
        )
        .bind(subdivision)
        .fetch_all(&self.db)
        .await
        .map_err(|e| Error::Storage(Box::new(e)))?;
        Ok(rows
            .into_iter()
            .map(|(code, name)| Municipality { code, name })
            .collect())
    }

    async fn municipality_in_subdivision(&self, code: i32, subdivision: &str) -> Result<bool> {
        let exists: (bool,) = sqlx::query_as(
            "SELECT EXISTS(SELECT 1 FROM municipio_ibge WHERE codigo_ibge = $1 AND uf = $2)",
        )
        .bind(code)
        .bind(subdivision)
        .fetch_one(&self.db)
        .await
        .map_err(|e| Error::Storage(Box::new(e)))?;
        Ok(exists.0)
    }
}

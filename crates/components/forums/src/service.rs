//! O serviço dos fóruns: resolução/materialização de caminhos, tópicos, votos,
//! comentários e o disparo de patamares — tudo transacional. Interações federadas
//! NUNCA entram na contagem que dispara (decisão do plano v3).

use std::sync::Arc;

use dsoc_core::ids::{CitizenId, OrgId};
use dsoc_core::{Clock, Error, Result};
use dsoc_db::Db;
use uuid::Uuid;

use crate::domain::{
    self, territorial_sections, thresholds_to_fire, NewTopic, Stance, TerritorialSection,
};
use crate::queries::{self, CommentRow, DispatchRow, ForumRow, TopicRow};

/// Uma seção-filha na árvore: materializada (linha real) ou virtual (template
/// territorial ainda sem tópicos — vira linha no primeiro uso).
#[derive(Debug, Clone)]
pub struct ChildEntry {
    /// Segmento do caminho.
    pub slug: String,
    /// Caminho completo.
    pub full_path: String,
    /// Nome de exibição.
    pub name: String,
    /// `true` quando ainda não materializada.
    pub virtual_section: bool,
}

/// Serviço dos fóruns.
#[derive(Clone)]
pub struct ForumService {
    db: Db,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for ForumService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForumService").finish_non_exhaustive()
    }
}

impl ForumService {
    /// Constrói com pool e clock explícitos (testes).
    #[must_use]
    pub fn new(db: Db, clock: Arc<dyn Clock>) -> Self {
        Self { db, clock }
    }

    /// Constrói a partir do estado compartilhado.
    #[must_use]
    pub fn from_state(state: &dsoc_app::AppState) -> Self {
        Self::new(state.db.clone(), state.clock.clone())
    }

    /// Resolve um caminho para o fórum, MATERIALIZANDO a seção territorial padrão
    /// quando o último segmento é um template (7 seções) sob um estado/município.
    ///
    /// Colisão conhecida: município homônimo de seção (ex.: Saúde/BA em `ba/saude`)
    /// tem precedência — a seção estadual homônima vive em `<uf>/<slug>-estado`.
    ///
    /// # Errors
    /// [`Error::NotFound`] quando o caminho não existe nem é materializável;
    /// [`Error::Validation`] para caminho malformado.
    pub async fn resolve_or_materialize(&self, org: OrgId, path: &str) -> Result<ForumRow> {
        let segments = domain::validate_path(path)?;
        let full_path = segments.join("/");
        match queries::get_forum_by_path(&self.db, org.as_uuid(), &full_path).await {
            Ok(row) => return Ok(row),
            Err(sqlx::Error::RowNotFound) => {}
            Err(e) => return Err(map_sqlx(e)),
        }
        // Não existe — só materializa se o ÚLTIMO segmento for seção template
        // sob um pai territorial existente.
        if segments.len() < 2 {
            return Err(Error::NotFound("fórum não encontrado".to_owned()));
        }
        let last: &str = segments.last().copied().unwrap_or_default();
        let parent_path = segments[..segments.len() - 1].join("/");
        let parent = queries::get_forum_by_path(&self.db, org.as_uuid(), &parent_path)
            .await
            .map_err(map_sqlx)?;
        let municipal = parent.esfera.as_deref() == Some("municipal");
        let estadual = parent.esfera.as_deref() == Some("estadual") && parent.parent_id.is_none();
        if !municipal && !estadual {
            return Err(Error::NotFound("fórum não encontrado".to_owned()));
        }
        // Aceita tanto o slug puro quanto a variante '-estado' (colisão com município).
        let base = last.strip_suffix("-estado").unwrap_or(last);
        let section: Option<TerritorialSection> = territorial_sections(municipal)
            .into_iter()
            .find(|s| s.slug == base);
        let Some(section) = section else {
            return Err(Error::NotFound("fórum não encontrado".to_owned()));
        };
        queries::insert_forum_idempotent(
            &self.db,
            Uuid::now_v7(),
            org.as_uuid(),
            Some(parent.id),
            last,
            &full_path,
            section.name,
            "institucional",
            parent.esfera.as_deref(),
            parent.uf.as_deref(),
            parent.municipio.as_deref(),
        )
        .await
        .map_err(map_sqlx)?;
        queries::get_forum_by_path(&self.db, org.as_uuid(), &full_path)
            .await
            .map_err(map_sqlx)
    }

    /// A árvore de um nível: fórum (quando `path` presente) + filhos materializados +
    /// seções virtuais do template territorial ainda não criadas.
    ///
    /// # Errors
    /// [`Error::NotFound`]/[`Error::Storage`] conforme a resolução.
    pub async fn tree(
        &self,
        org: OrgId,
        path: Option<&str>,
        esfera: Option<&str>,
    ) -> Result<(Option<ForumRow>, Vec<ChildEntry>)> {
        let forum = match path {
            Some(p) => Some(self.resolve_or_materialize(org, p).await?),
            None => None,
        };
        let rows = queries::list_children(
            &self.db,
            org.as_uuid(),
            forum.as_ref().map(|f| f.id),
            esfera,
            6000,
        )
        .await
        .map_err(map_sqlx)?;
        let mut children: Vec<ChildEntry> = rows
            .iter()
            .map(|r| ChildEntry {
                slug: r.slug.clone(),
                full_path: r.full_path.clone(),
                name: r.name.clone(),
                virtual_section: false,
            })
            .collect();
        // Seções virtuais do template para territoriais (estado raiz / município).
        if let Some(f) = &forum {
            let municipal = f.esfera.as_deref() == Some("municipal");
            let estadual = f.esfera.as_deref() == Some("estadual") && f.parent_id.is_none();
            if municipal || estadual {
                for s in territorial_sections(municipal) {
                    // Colisão com município homônimo: a seção estadual usa '-estado'.
                    let slug = if estadual
                        && rows
                            .iter()
                            .any(|r| r.slug == s.slug && r.esfera.as_deref() == Some("municipal"))
                    {
                        format!("{}-estado", s.slug)
                    } else {
                        s.slug.to_owned()
                    };
                    let taken = children.iter().any(|c| c.slug == slug);
                    if !taken {
                        children.push(ChildEntry {
                            full_path: format!("{}/{}", f.full_path, slug),
                            slug,
                            name: s.name.to_owned(),
                            virtual_section: true,
                        });
                    }
                }
            }
        }
        Ok((forum, children))
    }

    /// Cria um tópico em `path` (materializando a seção se preciso).
    ///
    /// # Errors
    /// Validação/NotFound/Storage conforme as etapas.
    pub async fn create_topic(
        &self,
        org: OrgId,
        path: &str,
        author: CitizenId,
        new: &NewTopic,
    ) -> Result<TopicRow> {
        let forum = self.resolve_or_materialize(org, path).await?;
        queries::insert_topic(
            &self.db,
            Uuid::now_v7(),
            forum.id,
            author.as_uuid(),
            &new.title,
            &new.body,
            self.clock.now(),
        )
        .await
        .map_err(map_sqlx)
    }

    /// Lista tópicos (`hot` = por score; senão recentes).
    ///
    /// # Errors
    /// [`Error::Storage`].
    pub async fn list_topics(
        &self,
        org: OrgId,
        path: &str,
        hot: bool,
        limit: i64,
        offset: i64,
    ) -> Result<(ForumRow, Vec<TopicRow>)> {
        let forum = self.resolve_or_materialize(org, path).await?;
        let topics =
            queries::list_topics(&self.db, forum.id, hot, limit.clamp(1, 100), offset.max(0))
                .await
                .map_err(map_sqlx)?;
        Ok((forum, topics))
    }

    /// Últimos tópicos de todos os fóruns (feed da home /f).
    ///
    /// # Errors
    /// [`Error::Storage`].
    pub async fn recent_topics(
        &self,
        org: OrgId,
        limit: i64,
    ) -> Result<Vec<queries::RecentTopicRow>> {
        queries::list_recent_topics(&self.db, org.as_uuid(), limit.clamp(1, 50))
            .await
            .map_err(map_sqlx)
    }

    /// Detalhe do tópico + comentários aprovados + recibos de envio.
    ///
    /// # Errors
    /// [`Error::NotFound`]/[`Error::Storage`].
    pub async fn get_topic(
        &self,
        id: Uuid,
    ) -> Result<(TopicRow, Vec<CommentRow>, Vec<DispatchRow>)> {
        let mut tx = self.db.begin().await.map_err(map_sqlx)?;
        let Some(topic) = queries::lock_topic(&mut *tx, id).await.map_err(map_sqlx)? else {
            return Err(Error::NotFound("tópico não encontrado".to_owned()));
        };
        let comments = queries::list_comments(&mut *tx, id, None, 200)
            .await
            .map_err(map_sqlx)?;
        let dispatches = queries::list_dispatches(&mut *tx, id)
            .await
            .map_err(map_sqlx)?;
        tx.commit().await.map_err(map_sqlx)?;
        Ok((topic, comments, dispatches))
    }

    /// Posição do cidadão (upsert; a favor/contra/ponderação) — recalcula
    /// contadores e dispara patamares pendentes.
    ///
    /// # Errors
    /// [`Error::NotFound`]/[`Error::Validation`]/[`Error::Storage`].
    pub async fn vote(
        &self,
        topic_id: Uuid,
        citizen: CitizenId,
        stance: Stance,
    ) -> Result<TopicRow> {
        let now = self.clock.now();
        let mut tx = self.db.begin().await.map_err(map_sqlx)?;
        let Some(topic) = queries::lock_topic(&mut *tx, topic_id)
            .await
            .map_err(map_sqlx)?
        else {
            return Err(Error::NotFound("tópico não encontrado".to_owned()));
        };
        queries::upsert_vote(&mut *tx, topic_id, citizen.as_uuid(), stance.as_str(), now)
            .await
            .map_err(map_sqlx)?;
        let updated = Self::after_interaction(&mut tx, &topic, now).await?;
        tx.commit().await.map_err(map_sqlx)?;
        Ok(updated)
    }

    /// Comentário local — com posição opcional (modelo do debate: argumento +
    /// posição juntos; a posição também registra/atualiza o voto do autor).
    /// Recalcula contadores e dispara patamares pendentes.
    ///
    /// # Errors
    /// [`Error::NotFound`]/[`Error::Validation`]/[`Error::Storage`].
    pub async fn comment(
        &self,
        topic_id: Uuid,
        citizen: CitizenId,
        body: &str,
        stance: Option<Stance>,
    ) -> Result<TopicRow> {
        let body = domain::validate_comment(body)?;
        let now = self.clock.now();
        let mut tx = self.db.begin().await.map_err(map_sqlx)?;
        let Some(topic) = queries::lock_topic(&mut *tx, topic_id)
            .await
            .map_err(map_sqlx)?
        else {
            return Err(Error::NotFound("tópico não encontrado".to_owned()));
        };
        queries::insert_local_comment(
            &mut *tx,
            Uuid::now_v7(),
            topic_id,
            citizen.as_uuid(),
            stance.map(Stance::as_str),
            &body,
            now,
        )
        .await
        .map_err(map_sqlx)?;
        if let Some(stance) = stance {
            queries::upsert_vote(&mut *tx, topic_id, citizen.as_uuid(), stance.as_str(), now)
                .await
                .map_err(map_sqlx)?;
        }
        let updated = Self::after_interaction(&mut tx, &topic, now).await?;
        tx.commit().await.map_err(map_sqlx)?;
        Ok(updated)
    }

    /// Pós-interação (sob o row lock): recontagem + patamares. O índice de patamar
    /// só avança quando o recibo é criado — fórum SEM e-mail curado fica pendente
    /// e dispara retroativamente quando a curadoria preencher o e-mail.
    async fn after_interaction(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        topic: &TopicRow,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<TopicRow> {
        let (interactions, _fed) = queries::refresh_topic_counters(&mut **tx, topic.id)
            .await
            .map_err(map_sqlx)?;

        let forum = sqlx::query!(
            r#"SELECT thresholds FROM forum WHERE id = $1"#,
            topic.forum_id
        )
        .fetch_one(&mut **tx)
        .await
        .map_err(map_sqlx)?;
        let next_idx = usize::try_from(topic.next_threshold_idx).unwrap_or(0);
        let (fire, new_idx) = thresholds_to_fire(interactions, &forum.thresholds, next_idx);
        if !fire.is_empty() {
            let email = queries::effective_contact_email(&mut **tx, topic.forum_id)
                .await
                .map_err(map_sqlx)?;
            if let Some(email) = email {
                for t in &fire {
                    queries::insert_dispatch(&mut **tx, Uuid::now_v7(), topic.id, *t, &email, now)
                        .await
                        .map_err(map_sqlx)?;
                }
                let _ = queries::advance_threshold_idx(
                    &mut **tx,
                    topic.id,
                    topic.next_threshold_idx,
                    i32::try_from(new_idx).unwrap_or(topic.next_threshold_idx),
                )
                .await
                .map_err(map_sqlx)?;
            }
        }

        let Some(updated) = queries::lock_topic(&mut **tx, topic.id)
            .await
            .map_err(map_sqlx)?
        else {
            return Err(Error::NotFound("tópico não encontrado".to_owned()));
        };
        Ok(updated)
    }
}

/// Mapeia falhas sqlx no modelo canônico (convenção CONTRIBUTING.md).
fn map_sqlx(err: sqlx::Error) -> Error {
    match err {
        sqlx::Error::RowNotFound => Error::NotFound("fórum não encontrado".to_owned()),
        sqlx::Error::Database(ref db) if db.is_unique_violation() => {
            Error::Conflict("registro já existe".to_owned())
        }
        sqlx::Error::Database(ref db) if db.is_foreign_key_violation() => {
            Error::Conflict("referência inexistente".to_owned())
        }
        other => Error::Storage(Box::new(other)),
    }
}

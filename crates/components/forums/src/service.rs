//! O serviço dos fóruns: resolução/materialização de caminhos, tópicos, votos,
//! comentários e o disparo de patamares — tudo transacional. Interações federadas
//! NUNCA entram na contagem que dispara (decisão do plano v3).

use std::sync::Arc;

use dsoc_core::ids::{CitizenId, OrgId};
use dsoc_core::{Clock, Error, Result};
use dsoc_db::Db;
use uuid::Uuid;

use crate::domain::{self, territorial_sections, NewTopic, Stance, TerritorialSection};
use crate::queries::{self, CommentRow, DispatchRow, ForumRow, TopicRow};

// --- Config do patamar proporcional dos fóruns (D3 do plano de crítica) ---
//
// O gatilho de encaminhamento ao gabinete deixa de ser o "10 pontos" fixo do
// ADR-0019 e passa a ser PROPORCIONAL ao eleitorado do território do fórum,
// pela MESMA fórmula das propostas (`dsoc_core::proportional_threshold`):
//
//     patamar = clamp( ceil(fração × eleitorado), piso, teto )
//
// A fração default (0,05%) é a mesma das propostas — UMA régua só (B1). O piso
// 10 preserva exatamente o comportamento do ADR-0019 (território sem dado, ou
// município pequeno, seguem exigindo 10). O teto evita alvo impossível numa
// capital/nacional. Tudo sobrescrevível por env; defaults à prova de falha.
const DEFAULT_FORUM_FRACTION: f64 = 0.0005;
const DEFAULT_FORUM_FLOOR: i64 = 10;
const DEFAULT_FORUM_CEIL: i64 = 10_000;

/// Privacidade graduada por tamanho de município (D5/D6). Abaixo deste
/// eleitorado, SÓ o agregado do tópico é público — a posição individual (quem
/// argumentou a favor/contra) não é atribuída, para não virar mapa de
/// retaliação em território pequeno + clientelista. Sobrescrevível por
/// `SMALL_MUNICIPALITY_ELECTORATE`. Só municípios caem abaixo disso (a menor UF
/// tem centenas de milhares de eleitores), então na prática é a régua municipal.
const DEFAULT_SMALL_MUNICIPALITY_ELECTORATE: i64 = 5_000;

/// Lê uma fração `(0,1]` de env; fora da faixa ou ausente → default (fail-safe).
fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|v: &f64| *v > 0.0 && *v <= 1.0)
        .unwrap_or(default)
}

/// Lê um inteiro positivo de env; ausente/inválido → default (fail-safe).
fn env_i64(key: &str, default: i64) -> i64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|v: &i64| *v > 0)
        .unwrap_or(default)
}

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

/// Detalhe de um tópico já enriquecido com os derivados do território:
/// patamar proporcional efetivo (D3) e privacidade agregada (D5/D6).
#[derive(Debug, Clone)]
pub struct TopicDetail {
    /// O tópico.
    pub topic: TopicRow,
    /// Comentários aprovados (locais + federados).
    pub comments: Vec<CommentRow>,
    /// Recibos de envio institucional.
    pub dispatches: Vec<DispatchRow>,
    /// Patamar de encaminhamento proporcional efetivo deste fórum (D3) — o
    /// score que o placar precisa cruzar para acionar o gabinete. A UI mostra
    /// "faltam N" a partir daqui, em vez do antigo 10 fixo.
    pub escalation_threshold: i64,
    /// Município pequeno (D5/D6): quando `true`, a atribuição individual de
    /// posição foi omitida — só o agregado é público.
    pub aggregate_only: bool,
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

    /// Detalhe do tópico + comentários aprovados + recibos de envio, mais os
    /// derivados do território: o patamar proporcional efetivo (D3) e o flag
    /// `aggregate_only` (D5/D6).
    ///
    /// # Errors
    /// [`Error::NotFound`]/[`Error::Storage`].
    pub async fn get_topic(&self, id: Uuid) -> Result<TopicDetail> {
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
        // Um lookup de eleitorado alimenta os dois derivados (D3 + D5/D6).
        let voters = queries::forum_territory_voters(&mut *tx, topic.forum_id)
            .await
            .map_err(map_sqlx)?;
        tx.commit().await.map_err(map_sqlx)?;
        let escalation_threshold = dsoc_core::proportional_threshold(
            voters,
            env_f64("FORUM_THRESHOLD_FRACTION", DEFAULT_FORUM_FRACTION),
            env_i64("FORUM_THRESHOLD_FLOOR", DEFAULT_FORUM_FLOOR),
            env_i64("FORUM_THRESHOLD_CEIL", DEFAULT_FORUM_CEIL),
        );
        let aggregate_only = dsoc_core::is_small_electorate(
            voters,
            env_i64(
                "SMALL_MUNICIPALITY_ELECTORATE",
                DEFAULT_SMALL_MUNICIPALITY_ELECTORATE,
            ),
        );
        Ok(TopicDetail {
            topic,
            comments,
            dispatches,
            escalation_threshold,
            aggregate_only,
        })
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

    /// Posição num ARGUMENTO (0545, estilo StackOverflow) — upsert; recalcula
    /// os contadores do argumento e do tópico (voto em argumento é interação
    /// contável) e dispara patamares pendentes.
    ///
    /// # Errors
    /// [`Error::NotFound`]/[`Error::Storage`].
    pub async fn vote_comment(
        &self,
        comment_id: Uuid,
        citizen: CitizenId,
        stance: Stance,
    ) -> Result<(queries::CommentRow, TopicRow)> {
        let now = self.clock.now();
        let mut tx = self.db.begin().await.map_err(map_sqlx)?;
        let comment = queries::get_comment(&mut *tx, comment_id)
            .await
            .map_err(|e| match e {
                sqlx::Error::RowNotFound => Error::NotFound("argumento não encontrado".to_owned()),
                other => map_sqlx(other),
            })?;
        let Some(topic) = queries::lock_topic(&mut *tx, comment.topic_id)
            .await
            .map_err(map_sqlx)?
        else {
            return Err(Error::NotFound("tópico não encontrado".to_owned()));
        };
        // Karma (ADR-0019): o AUTOR do argumento ganha/perde reputação conforme o voto que recebe
        // (SO: favor=+10, contra=−2). Delta = valor(novo) − valor(anterior) pra cobrir troca de voto.
        // Voto no próprio comentário não gera karma (anti-self-vote, estilo SO).
        let prev_stance = queries::comment_vote_stance(&mut *tx, comment_id, citizen.as_uuid())
            .await
            .map_err(map_sqlx)?;
        queries::upsert_comment_vote(
            &mut *tx,
            comment_id,
            citizen.as_uuid(),
            stance.as_str(),
            now,
        )
        .await
        .map_err(map_sqlx)?;
        if let Some(author) = comment.author_id {
            if author != citizen.as_uuid() {
                let delta = queries::karma_value(stance.as_str())
                    - prev_stance.as_deref().map_or(0, queries::karma_value);
                queries::add_citizen_karma(&mut *tx, author, delta)
                    .await
                    .map_err(map_sqlx)?;
            }
        }
        queries::refresh_comment_counters(&mut *tx, comment_id)
            .await
            .map_err(map_sqlx)?;
        let updated_topic = Self::after_interaction(&mut tx, &topic, now).await?;
        let updated_comment = queries::get_comment(&mut *tx, comment_id)
            .await
            .map_err(map_sqlx)?;
        tx.commit().await.map_err(map_sqlx)?;
        Ok((updated_comment, updated_topic))
    }

    /// Pós-interação (sob o row lock): recontagem + patamares. O índice de patamar
    /// só avança quando o recibo é criado — fórum SEM e-mail curado fica pendente
    /// e dispara retroativamente quando a curadoria preencher o e-mail.
    async fn after_interaction(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        topic: &TopicRow,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<TopicRow> {
        let (_interactions, _fed, score) = queries::refresh_topic_counters(&mut **tx, topic.id)
            .await
            .map_err(map_sqlx)?;

        // ADR-0019 + D3: encaminha ao gabinete quando o PLACAR (pontos com sinal) cruza o patamar
        // — uma única vez. `next_threshold_idx` vira flag: 0 = ainda não escalou, 1 = escalou.
        // O patamar agora é PROPORCIONAL ao eleitorado do território do fórum (piso 10 = o antigo
        // corte fixo; teto evita alvo impossível em capital/nacional): mesmo esforço relativo em
        // Roraima e em SP, e volume bruto deixa de ser trivialmente gameável. Só demanda com apoio
        // líquido (score ≥ patamar) escala; controverso (líquido baixo) não.
        // (Fórum sem e-mail curado fica pendente e dispara quando a curadoria preencher.)
        let escalation = Self::forum_escalation_threshold(&mut **tx, topic.forum_id).await?;
        if score >= escalation && topic.next_threshold_idx == 0 {
            let email = queries::effective_contact_email(&mut **tx, topic.forum_id)
                .await
                .map_err(map_sqlx)?;
            if let Some(email) = email {
                // O recibo registra o patamar REALMENTE cruzado (proporcional), não o "10" fixo.
                let recorded = i32::try_from(escalation).unwrap_or(i32::MAX);
                queries::insert_dispatch(
                    &mut **tx,
                    Uuid::now_v7(),
                    topic.id,
                    recorded,
                    &email,
                    now,
                )
                .await
                .map_err(map_sqlx)?;
                let _ = queries::advance_threshold_idx(
                    &mut **tx,
                    topic.id,
                    topic.next_threshold_idx,
                    1,
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

    /// Patamar de encaminhamento proporcional ao eleitorado do território do
    /// fórum (D3). `clamp(ceil(fração × eleitorado), piso, teto)`. Território
    /// sem esfera/eleitorado → piso (fail-safe; nunca desliga o gatilho).
    ///
    /// # Errors
    /// [`Error::Storage`] se a consulta ao eleitorado falhar.
    async fn forum_escalation_threshold(
        executor: impl sqlx::PgExecutor<'_>,
        forum_id: Uuid,
    ) -> Result<i64> {
        let voters = queries::forum_territory_voters(executor, forum_id)
            .await
            .map_err(map_sqlx)?;
        let fraction = env_f64("FORUM_THRESHOLD_FRACTION", DEFAULT_FORUM_FRACTION);
        let floor = env_i64("FORUM_THRESHOLD_FLOOR", DEFAULT_FORUM_FLOOR);
        let ceil = env_i64("FORUM_THRESHOLD_CEIL", DEFAULT_FORUM_CEIL);
        Ok(dsoc_core::proportional_threshold(voters, fraction, floor, ceil))
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

//! Persistência dos fóruns. Todo statement é `sqlx::query!` compile-time-checked
//! (PLAN.md princípio 3): sem ORM, keyset pagination, UPDATEs guardados por estado
//! esperado para idempotência sob at-least-once.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Uma linha de fórum.
#[derive(Debug, Clone)]
pub struct ForumRow {
    /// Id do fórum.
    pub id: Uuid,
    /// Organização dona.
    pub org_id: Uuid,
    /// Pai na hierarquia, se houver.
    pub parent_id: Option<Uuid>,
    /// Segmento do caminho.
    pub slug: String,
    /// Caminho completo (`sp/santos/saude`).
    pub full_path: String,
    /// Nome de exibição.
    pub name: String,
    /// Descrição.
    pub description: String,
    /// `institucional` | `governanca` | `comunitario`.
    pub kind: String,
    /// Esfera federativa, se territorial.
    pub esfera: Option<String>,
    /// UF, se territorial.
    pub uf: Option<String>,
    /// Município, se territorial.
    pub municipio: Option<String>,
    /// E-mail responsável (herda do pai quando NULL).
    pub contact_email: Option<String>,
    /// Logo do fórum (0543).
    pub avatar_url: Option<String>,
    /// Capa do fórum (0543).
    pub banner_url: Option<String>,
    /// Patamares de envio (interações contáveis), ordem crescente.
    pub thresholds: Vec<i32>,
    /// Criação.
    pub created_at: DateTime<Utc>,
}

/// Uma linha de tópico.
#[derive(Debug, Clone)]
pub struct TopicRow {
    /// Id do tópico.
    pub id: Uuid,
    /// Fórum dono.
    pub forum_id: Uuid,
    /// Autor (cidadão local — criação é sempre local).
    pub author_id: Uuid,
    /// Título.
    pub title: String,
    /// Corpo.
    pub body: String,
    /// Interações contáveis (votos + comentários locais).
    pub interaction_count: i64,
    /// Interações federadas (nunca disparam patamar).
    pub federated_interaction_count: i64,
    /// Saldo favor - contra (ordenação "em alta").
    pub score: i64,
    /// Posições a favor (0544).
    pub favor_count: i64,
    /// Posições contra (0544).
    pub contra_count: i64,
    /// Ponderações (0544).
    pub ponderacao_count: i64,
    /// Total de comentários aprovados.
    pub comment_count: i64,
    /// Próximo patamar a disparar (índice em `forum.thresholds`).
    pub next_threshold_idx: i32,
    /// Criação.
    pub created_at: DateTime<Utc>,
}

/// Um comentário (local ou federado).
#[derive(Debug, Clone)]
pub struct CommentRow {
    /// Id.
    pub id: Uuid,
    /// Tópico dono.
    pub topic_id: Uuid,
    /// Autor local, se local.
    pub author_id: Option<Uuid>,
    /// Handle do ator remoto, se federado.
    pub remote_handle: Option<String>,
    /// Federado?
    pub federated: bool,
    /// Posição declarada junto do argumento (NULL = sem posição / federado).
    pub stance: Option<String>,
    /// Votos a favor DESTE argumento (0545).
    pub favor_count: i64,
    /// Votos contra.
    pub contra_count: i64,
    /// Ponderações (vestigial, sempre 0 pós-ADR-0019).
    pub ponderacao_count: i64,
    /// Karma (reputação SO) do autor local, se local (ADR-0019). `None` = federado/sem autor.
    pub author_karma: Option<i32>,
    /// Corpo.
    pub body: String,
    /// Criação.
    pub created_at: DateTime<Utc>,
}

/// Um argumento-ponte (D8.2): o comentário mais os ENDOSSOS cruzados por lado do
/// tópico. `favor_side`/`contra_side` = quantos endossantes deste argumento
/// (voto `favor` no comentário) votaram, respectivamente, `favor`/`contra` NO
/// TÓPICO. O bridge score é derivado por [`crate::domain::bridge_score`].
#[derive(Debug, Clone)]
pub struct BridgeCommentRow {
    /// O comentário/argumento (local, aprovado).
    pub comment: CommentRow,
    /// Endossantes cuja posição no tópico é `favor`.
    pub favor_side: i64,
    /// Endossantes cuja posição no tópico é `contra`.
    pub contra_side: i64,
}

/// Um recibo de envio institucional por patamar.
#[derive(Debug, Clone)]
pub struct DispatchRow {
    /// Id.
    pub id: Uuid,
    /// Patamar cruzado.
    pub threshold: i32,
    /// Destino do envio.
    pub contact_email: String,
    /// Quando o e-mail saiu (NULL = pendente do worker).
    pub sent_at: Option<DateTime<Utc>>,
    /// Criação (momento do cruzamento).
    pub created_at: DateTime<Utc>,
}

#[allow(clippy::too_many_arguments)]
fn map_forum(
    id: Uuid,
    org_id: Uuid,
    parent_id: Option<Uuid>,
    slug: String,
    full_path: String,
    name: String,
    description: String,
    kind: String,
    esfera: Option<String>,
    uf: Option<String>,
    municipio: Option<String>,
    contact_email: Option<String>,
    avatar_url: Option<String>,
    banner_url: Option<String>,
    thresholds: Vec<i32>,
    created_at: DateTime<Utc>,
) -> ForumRow {
    ForumRow {
        id,
        org_id,
        parent_id,
        slug,
        full_path,
        name,
        description,
        kind,
        esfera,
        uf,
        municipio,
        contact_email,
        avatar_url,
        banner_url,
        thresholds,
        created_at,
    }
}

/// Busca um fórum pelo caminho completo.
///
/// # Errors
/// Propaga o `sqlx::Error` (incluindo `RowNotFound`).
pub async fn get_forum_by_path(
    executor: impl sqlx::PgExecutor<'_>,
    org_id: Uuid,
    full_path: &str,
) -> Result<ForumRow, sqlx::Error> {
    let r = sqlx::query!(
        r#"SELECT id, org_id, parent_id, slug, full_path, name, description, kind,
                  esfera, uf, municipio, contact_email, avatar_url, banner_url,
                  thresholds, created_at
           FROM forum WHERE org_id = $1 AND full_path = $2 AND hidden_at IS NULL"#,
        org_id,
        full_path,
    )
    .fetch_one(executor)
    .await?;
    Ok(map_forum(
        r.id,
        r.org_id,
        r.parent_id,
        r.slug,
        r.full_path,
        r.name,
        r.description,
        r.kind,
        r.esfera,
        r.uf,
        r.municipio,
        r.contact_email,
        r.avatar_url,
        r.banner_url,
        r.thresholds,
        r.created_at,
    ))
}

/// Lista os filhos diretos de um fórum (ou raízes por esfera quando `parent_id` é NULL).
///
/// # Errors
/// Propaga o `sqlx::Error`.
pub async fn list_children(
    executor: impl sqlx::PgExecutor<'_>,
    org_id: Uuid,
    parent_id: Option<Uuid>,
    esfera: Option<&str>,
    limit: i64,
) -> Result<Vec<ForumRow>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT id, org_id, parent_id, slug, full_path, name, description, kind,
                  esfera, uf, municipio, contact_email, avatar_url, banner_url,
                  thresholds, created_at
           FROM forum
           WHERE org_id = $1 AND hidden_at IS NULL
             AND parent_id IS NOT DISTINCT FROM $2
             AND ($3::text IS NULL OR esfera = $3)
           ORDER BY slug
           LIMIT $4"#,
        org_id,
        parent_id,
        esfera,
        limit,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            map_forum(
                r.id,
                r.org_id,
                r.parent_id,
                r.slug,
                r.full_path,
                r.name,
                r.description,
                r.kind,
                r.esfera,
                r.uf,
                r.municipio,
                r.contact_email,
                r.avatar_url,
                r.banner_url,
                r.thresholds,
                r.created_at,
            )
        })
        .collect())
}

/// Insere um fórum (materialização preguiçosa das seções territoriais).
/// Idempotente sob corrida: `ON CONFLICT (org_id, full_path) DO NOTHING`.
///
/// # Errors
/// Propaga o `sqlx::Error`.
#[allow(clippy::too_many_arguments)]
pub async fn insert_forum_idempotent(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    org_id: Uuid,
    parent_id: Option<Uuid>,
    slug: &str,
    full_path: &str,
    name: &str,
    kind: &str,
    esfera: Option<&str>,
    uf: Option<&str>,
    municipio: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"INSERT INTO forum (id, org_id, parent_id, slug, full_path, name, kind,
                              esfera, uf, municipio, created_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, now())
           ON CONFLICT (org_id, full_path) DO NOTHING"#,
        id,
        org_id,
        parent_id,
        slug,
        full_path,
        name,
        kind,
        esfera,
        uf,
        municipio,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// Insere um tópico.
///
/// # Errors
/// Propaga o `sqlx::Error`.
pub async fn insert_topic(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    forum_id: Uuid,
    author_id: Uuid,
    title: &str,
    body: &str,
    created_at: DateTime<Utc>,
) -> Result<TopicRow, sqlx::Error> {
    let r = sqlx::query!(
        r#"INSERT INTO forum_topic (id, forum_id, author_id, title, body, created_at)
           VALUES ($1, $2, $3, $4, $5, $6)
           RETURNING id, forum_id, author_id, title, body, interaction_count,
                     federated_interaction_count, score, favor_count, contra_count,
                     ponderacao_count, comment_count, next_threshold_idx, created_at"#,
        id,
        forum_id,
        author_id,
        title,
        body,
        created_at,
    )
    .fetch_one(executor)
    .await?;
    Ok(TopicRow {
        id: r.id,
        forum_id: r.forum_id,
        author_id: r.author_id,
        title: r.title,
        body: r.body,
        interaction_count: r.interaction_count,
        federated_interaction_count: r.federated_interaction_count,
        score: r.score,
        favor_count: r.favor_count,
        contra_count: r.contra_count,
        ponderacao_count: r.ponderacao_count,
        comment_count: r.comment_count,
        next_threshold_idx: r.next_threshold_idx,
        created_at: r.created_at,
    })
}

/// Tranca um tópico `FOR UPDATE` (janela TOCTOU dos contadores/patamares).
///
/// # Errors
/// Propaga o `sqlx::Error`.
pub async fn lock_topic(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
) -> Result<Option<TopicRow>, sqlx::Error> {
    let r = sqlx::query!(
        r#"SELECT id, forum_id, author_id, title, body, interaction_count,
                  federated_interaction_count, score, favor_count, contra_count,
                  ponderacao_count, comment_count, next_threshold_idx, created_at
           FROM forum_topic WHERE id = $1 AND hidden_at IS NULL FOR UPDATE"#,
        id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(r.map(|r| TopicRow {
        id: r.id,
        forum_id: r.forum_id,
        author_id: r.author_id,
        title: r.title,
        body: r.body,
        interaction_count: r.interaction_count,
        federated_interaction_count: r.federated_interaction_count,
        score: r.score,
        favor_count: r.favor_count,
        contra_count: r.contra_count,
        ponderacao_count: r.ponderacao_count,
        comment_count: r.comment_count,
        next_threshold_idx: r.next_threshold_idx,
        created_at: r.created_at,
    }))
}

/// Lista tópicos de um fórum: `hot` (score desc) ou recentes (id desc).
///
/// # Errors
/// Propaga o `sqlx::Error`.
pub async fn list_topics(
    executor: impl sqlx::PgExecutor<'_>,
    forum_id: Uuid,
    hot: bool,
    limit: i64,
    offset: i64,
) -> Result<Vec<TopicRow>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT id, forum_id, author_id, title, body, interaction_count,
                  federated_interaction_count, score, favor_count, contra_count,
                  ponderacao_count, comment_count, next_threshold_idx, created_at
           FROM forum_topic
           WHERE forum_id = $1 AND hidden_at IS NULL
           ORDER BY CASE WHEN $2 THEN score END DESC NULLS LAST, id DESC
           LIMIT $3 OFFSET $4"#,
        forum_id,
        hot,
        limit,
        offset,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| TopicRow {
            id: r.id,
            forum_id: r.forum_id,
            author_id: r.author_id,
            title: r.title,
            body: r.body,
            interaction_count: r.interaction_count,
            federated_interaction_count: r.federated_interaction_count,
            score: r.score,
            favor_count: r.favor_count,
            contra_count: r.contra_count,
            ponderacao_count: r.ponderacao_count,
            comment_count: r.comment_count,
            next_threshold_idx: r.next_threshold_idx,
            created_at: r.created_at,
        })
        .collect())
}

/// Um tópico recente com o fórum de origem (feed da home /f).
#[derive(Debug, Clone)]
pub struct RecentTopicRow {
    /// Id do tópico.
    pub id: Uuid,
    /// Título.
    pub title: String,
    /// Saldo favor - contra.
    pub score: i64,
    /// Posições a favor.
    pub favor_count: i64,
    /// Posições contra.
    pub contra_count: i64,
    /// Ponderações.
    pub ponderacao_count: i64,
    /// Interações contáveis.
    pub interaction_count: i64,
    /// Comentários aprovados.
    pub comment_count: i64,
    /// Criação.
    pub created_at: DateTime<Utc>,
    /// Caminho do fórum (`sp/santos/saude`).
    pub forum_path: String,
    /// Nome do fórum.
    pub forum_name: String,
}

/// Últimos tópicos de TODOS os fóruns (feed da home), mais novos primeiro.
///
/// # Errors
/// Propaga o `sqlx::Error`.
pub async fn list_recent_topics(
    executor: impl sqlx::PgExecutor<'_>,
    org_id: Uuid,
    limit: i64,
) -> Result<Vec<RecentTopicRow>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT t.id, t.title, t.score, t.favor_count, t.contra_count,
                  t.ponderacao_count, t.interaction_count, t.comment_count, t.created_at,
                  f.full_path AS forum_path, f.name AS forum_name
           FROM forum_topic t
           JOIN forum f ON f.id = t.forum_id
           WHERE t.hidden_at IS NULL AND f.hidden_at IS NULL AND f.org_id = $1
           ORDER BY t.id DESC
           LIMIT $2"#,
        org_id,
        limit,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| RecentTopicRow {
            id: r.id,
            title: r.title,
            score: r.score,
            favor_count: r.favor_count,
            contra_count: r.contra_count,
            ponderacao_count: r.ponderacao_count,
            interaction_count: r.interaction_count,
            comment_count: r.comment_count,
            created_at: r.created_at,
            forum_path: r.forum_path,
            forum_name: r.forum_name,
        })
        .collect())
}

/// Upsert da posição do cidadão (uma linha por par tópico-cidadão; mudar de
/// posição sobrescreve — a data original do primeiro voto é preservada).
///
/// # Errors
/// Propaga o `sqlx::Error`.
pub async fn upsert_vote(
    executor: impl sqlx::PgExecutor<'_>,
    topic_id: Uuid,
    citizen_id: Uuid,
    stance: &str,
    created_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"INSERT INTO forum_topic_vote (topic_id, citizen_id, stance, created_at)
           VALUES ($1, $2, $3, $4)
           ON CONFLICT (topic_id, citizen_id) DO UPDATE SET stance = EXCLUDED.stance"#,
        topic_id,
        citizen_id,
        stance,
        created_at,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// Insere um comentário LOCAL (aprovado de nascença).
///
/// # Errors
/// Propaga o `sqlx::Error`.
pub async fn insert_local_comment(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    topic_id: Uuid,
    author_id: Uuid,
    stance: Option<&str>,
    body: &str,
    created_at: DateTime<Utc>,
) -> Result<CommentRow, sqlx::Error> {
    let r = sqlx::query!(
        r#"INSERT INTO forum_topic_comment
               (id, topic_id, author_id, federated, stance, body, created_at)
           VALUES ($1, $2, $3, false, $4, $5, $6)
           RETURNING id, topic_id, author_id, remote_handle, federated, stance,
                     favor_count, contra_count, ponderacao_count, body, created_at,
                     (SELECT ci.karma FROM citizen ci WHERE ci.id = author_id) AS author_karma"#,
        id,
        topic_id,
        author_id,
        stance,
        body,
        created_at,
    )
    .fetch_one(executor)
    .await?;
    Ok(CommentRow {
        id: r.id,
        topic_id: r.topic_id,
        author_id: r.author_id,
        remote_handle: r.remote_handle,
        federated: r.federated,
        stance: r.stance,
        favor_count: r.favor_count,
        contra_count: r.contra_count,
        ponderacao_count: r.ponderacao_count,
        author_karma: r.author_karma,
        body: r.body,
        created_at: r.created_at,
    })
}

/// Lista comentários APROVADOS de um tópico (mais antigos primeiro, keyset).
///
/// # Errors
/// Propaga o `sqlx::Error`.
pub async fn list_comments(
    executor: impl sqlx::PgExecutor<'_>,
    topic_id: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<CommentRow>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT id, topic_id, author_id, remote_handle, federated, stance,
                  favor_count, contra_count, ponderacao_count, body, created_at,
                  (SELECT ci.karma FROM citizen ci WHERE ci.id = author_id) AS author_karma
           FROM forum_topic_comment
           WHERE topic_id = $1 AND moderation = 'approved' AND hidden_at IS NULL
             AND ($2::uuid IS NULL OR id > $2)
           ORDER BY id
           LIMIT $3"#,
        topic_id,
        after,
        limit,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| CommentRow {
            id: r.id,
            topic_id: r.topic_id,
            author_id: r.author_id,
            remote_handle: r.remote_handle,
            federated: r.federated,
            stance: r.stance,
            favor_count: r.favor_count,
            contra_count: r.contra_count,
            ponderacao_count: r.ponderacao_count,
            author_karma: r.author_karma,
            body: r.body,
            created_at: r.created_at,
        })
        .collect())
}

/// Busca um comentário aprovado (para votar num argumento).
///
/// # Errors
/// Propaga o `sqlx::Error` (incluindo `RowNotFound`).
pub async fn get_comment(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
) -> Result<CommentRow, sqlx::Error> {
    let r = sqlx::query!(
        r#"SELECT id, topic_id, author_id, remote_handle, federated, stance,
                  favor_count, contra_count, ponderacao_count, body, created_at,
                  (SELECT ci.karma FROM citizen ci WHERE ci.id = author_id) AS author_karma
           FROM forum_topic_comment WHERE id = $1 AND moderation = 'approved'"#,
        id,
    )
    .fetch_one(executor)
    .await?;
    Ok(CommentRow {
        id: r.id,
        topic_id: r.topic_id,
        author_id: r.author_id,
        remote_handle: r.remote_handle,
        federated: r.federated,
        stance: r.stance,
        favor_count: r.favor_count,
        contra_count: r.contra_count,
        ponderacao_count: r.ponderacao_count,
        author_karma: r.author_karma,
        body: r.body,
        created_at: r.created_at,
    })
}

/// Lista os ARGUMENTOS-PONTE candidatos de um tópico (D8.2): comentários locais
/// aprovados que recebem endosso (`forum_comment_vote.stance = 'favor'`) de
/// cidadãos posicionados em AMBOS os lados do tópico. Cruza cada endossante com
/// sua posição no tópico (`forum_topic_vote`) e agrega por lado. O `HAVING`
/// já descarta quem só tem apoio de um lado — a ordenação/corte por bridge score
/// fica no chamador (a fórmula mora em `domain::bridge_score`, testada).
///
/// Nota: só posições `favor`/`contra` no tópico contam; endossantes que votaram
/// no argumento mas não se posicionaram no tópico não entram na conta de ponte.
///
/// # Errors
/// Propaga o `sqlx::Error`.
pub async fn list_bridge_comments(
    executor: impl sqlx::PgExecutor<'_>,
    topic_id: Uuid,
) -> Result<Vec<BridgeCommentRow>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT c.id, c.topic_id, c.author_id, c.remote_handle, c.federated, c.stance,
                  c.favor_count, c.contra_count, c.ponderacao_count, c.body, c.created_at,
                  (SELECT ci.karma FROM citizen ci WHERE ci.id = c.author_id) AS author_karma,
                  COUNT(*) FILTER (WHERE tv.stance = 'favor')  AS "favor_side!",
                  COUNT(*) FILTER (WHERE tv.stance = 'contra') AS "contra_side!"
           FROM forum_topic_comment c
           JOIN forum_comment_vote cv
             ON cv.comment_id = c.id AND cv.stance = 'favor'
           JOIN forum_topic_vote tv
             ON tv.topic_id = c.topic_id AND tv.citizen_id = cv.citizen_id
          WHERE c.topic_id = $1
            AND c.moderation = 'approved'
            AND c.hidden_at IS NULL
            AND NOT c.federated
          GROUP BY c.id
         HAVING COUNT(*) FILTER (WHERE tv.stance = 'favor')  > 0
            AND COUNT(*) FILTER (WHERE tv.stance = 'contra') > 0"#,
        topic_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| BridgeCommentRow {
            comment: CommentRow {
                id: r.id,
                topic_id: r.topic_id,
                author_id: r.author_id,
                remote_handle: r.remote_handle,
                federated: r.federated,
                stance: r.stance,
                favor_count: r.favor_count,
                contra_count: r.contra_count,
                ponderacao_count: r.ponderacao_count,
                author_karma: r.author_karma,
                body: r.body,
                created_at: r.created_at,
            },
            favor_side: r.favor_side,
            contra_side: r.contra_side,
        })
        .collect())
}

/// Upsert da posição do cidadão num ARGUMENTO (0545) — uma por par, mutável.
///
/// # Errors
/// Propaga o `sqlx::Error`.
pub async fn upsert_comment_vote(
    executor: impl sqlx::PgExecutor<'_>,
    comment_id: Uuid,
    citizen_id: Uuid,
    stance: &str,
    created_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"INSERT INTO forum_comment_vote (comment_id, citizen_id, stance, created_at)
           VALUES ($1, $2, $3, $4)
           ON CONFLICT (comment_id, citizen_id) DO UPDATE SET stance = EXCLUDED.stance"#,
        comment_id,
        citizen_id,
        stance,
        created_at,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// Recalcula os contadores de um ARGUMENTO a partir dos votos (0545).
///
/// # Errors
/// Propaga o `sqlx::Error`.
pub async fn refresh_comment_counters(
    executor: impl sqlx::PgExecutor<'_>,
    comment_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"UPDATE forum_topic_comment c SET
             favor_count = v."favor!",
             contra_count = v."contra!"
           FROM (SELECT COUNT(*) FILTER (WHERE stance = 'favor') AS "favor!",
                        COUNT(*) FILTER (WHERE stance = 'contra') AS "contra!"
                   FROM forum_comment_vote WHERE comment_id = $1) v
           WHERE c.id = $1"#,
        comment_id,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// Stance do voto atual de `citizen` no comentário `comment_id`, se houver (`None` = não votou).
/// Usado pra computar o delta de karma quando o voto muda (ADR-0019).
pub async fn comment_vote_stance(
    executor: impl sqlx::PgExecutor<'_>,
    comment_id: Uuid,
    citizen: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let r = sqlx::query_scalar!(
        r#"SELECT stance FROM forum_comment_vote WHERE comment_id = $1 AND citizen_id = $2"#,
        comment_id,
        citizen,
    )
    .fetch_optional(executor)
    .await?;
    Ok(r)
}

/// Soma `delta` ao karma do cidadão (reputação estilo SO, ADR-0019). Pode ser negativo.
pub async fn add_citizen_karma(
    executor: impl sqlx::PgExecutor<'_>,
    citizen: Uuid,
    delta: i32,
) -> Result<(), sqlx::Error> {
    if delta == 0 {
        return Ok(());
    }
    sqlx::query!(
        r#"UPDATE citizen SET karma = karma + $2 WHERE id = $1"#,
        citizen,
        delta,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// Valor de karma de um voto em comentário por stance (SO: favor=+10, contra=−2).
#[must_use]
pub fn karma_value(stance: &str) -> i32 {
    match stance {
        "favor" => 10,
        "contra" => -2,
        _ => 0,
    }
}

/// Recalcula os contadores do tópico a partir das tabelas-fonte (sob o row lock):
/// posições por stance, score = favor - contra; interações contáveis = votos +
/// comentários locais aprovados + votos em argumentos (todos locais por FK);
/// federadas = comentários federados aprovados.
/// Retorna (interactions, federated).
///
/// # Errors
/// Propaga o `sqlx::Error`.
pub async fn refresh_topic_counters(
    executor: impl sqlx::PgExecutor<'_>,
    topic_id: Uuid,
) -> Result<(i64, i64, i64), sqlx::Error> {
    // Placar por PONTOS (ADR-0019, com sinal): base por votante (±2 se comentou, ±1 se só votou) +
    // amplificação dos votos nos argumentos (favor↑+2/↓−1, contra↑−2/↓+1). favor_count/contra_count
    // seguem sendo a CONTAGEM de votos por lado (exibição); `score` é o placar com sinal.
    let r = sqlx::query!(
        r#"UPDATE forum_topic t SET
             favor_count = v."favor!",
             contra_count = v."contra!",
             score = pos."base!" + amp."amp!",
             comment_count = c."local!" + c."fede!",
             interaction_count = v."votes!" + c."local!" + cv."cvotes!",
             federated_interaction_count = c."fede!"
           FROM
             (SELECT COUNT(*) FILTER (WHERE stance = 'favor') AS "favor!",
                     COUNT(*) FILTER (WHERE stance = 'contra') AS "contra!",
                     COUNT(*) AS "votes!"
                FROM forum_topic_vote WHERE topic_id = $1) v,
             (SELECT COUNT(*) FILTER (WHERE NOT federated) AS "local!",
                     COUNT(*) FILTER (WHERE federated) AS "fede!"
                FROM forum_topic_comment
               WHERE topic_id = $1 AND moderation = 'approved') c,
             (SELECT COUNT(*) AS "cvotes!"
                FROM forum_comment_vote fcv
                JOIN forum_topic_comment fc ON fc.id = fcv.comment_id
               WHERE fc.topic_id = $1) cv,
             -- base por votante: ±2 se a pessoa TAMBÉM comentou (argumentou), ±1 se só votou.
             (SELECT COALESCE(SUM(
                       CASE WHEN tv.stance = 'favor' THEN
                              CASE WHEN EXISTS (SELECT 1 FROM forum_topic_comment fc2
                                     WHERE fc2.topic_id = tv.topic_id AND fc2.author_id = tv.citizen_id
                                       AND NOT fc2.federated AND fc2.moderation = 'approved')
                                   THEN 2 ELSE 1 END
                            ELSE
                              CASE WHEN EXISTS (SELECT 1 FROM forum_topic_comment fc2
                                     WHERE fc2.topic_id = tv.topic_id AND fc2.author_id = tv.citizen_id
                                       AND NOT fc2.federated AND fc2.moderation = 'approved')
                                   THEN -2 ELSE -1 END
                       END), 0)::bigint AS "base!"
                FROM forum_topic_vote tv WHERE tv.topic_id = $1) pos,
             -- amplificação: voto no argumento × stance do argumento (matriz ADR-0019).
             (SELECT COALESCE(SUM(
                       CASE
                         WHEN fc.stance = 'favor'  AND fcv.stance = 'favor'  THEN 2
                         WHEN fc.stance = 'favor'  AND fcv.stance = 'contra' THEN -1
                         WHEN fc.stance = 'contra' AND fcv.stance = 'favor'  THEN -2
                         WHEN fc.stance = 'contra' AND fcv.stance = 'contra' THEN 1
                         ELSE 0
                       END), 0)::bigint AS "amp!"
                FROM forum_comment_vote fcv
                JOIN forum_topic_comment fc ON fc.id = fcv.comment_id
               WHERE fc.topic_id = $1 AND fc.moderation = 'approved' AND NOT fc.federated) amp
           WHERE t.id = $1
           RETURNING t.interaction_count, t.federated_interaction_count, t.score"#,
        topic_id,
    )
    .fetch_one(executor)
    .await?;
    Ok((r.interaction_count, r.federated_interaction_count, r.score))
}

/// Avança o índice de patamar, **guardado** pelo valor anterior esperado (1x por
/// patamar mesmo sob corrida). Retorna linhas afetadas (0 = já avançado).
///
/// # Errors
/// Propaga o `sqlx::Error`.
pub async fn advance_threshold_idx(
    executor: impl sqlx::PgExecutor<'_>,
    topic_id: Uuid,
    expected_idx: i32,
    new_idx: i32,
) -> Result<u64, sqlx::Error> {
    let res = sqlx::query!(
        r#"UPDATE forum_topic SET next_threshold_idx = $3
           WHERE id = $1 AND next_threshold_idx = $2"#,
        topic_id,
        expected_idx,
        new_idx,
    )
    .execute(executor)
    .await?;
    Ok(res.rows_affected())
}

/// Registra o recibo de envio de um patamar. `mandate_id` discrimina o
/// destinatário (B1): `None` = contato curado da seção (comportamento atual);
/// `Some` = um gabinete alvo — um tópico direcionado a N gabinetes registra N
/// recibos no mesmo patamar. O UNIQUE `(topic_id, threshold, mandate_id)`
/// (NULLS NOT DISTINCT, migration 0666) garante 1x por (patamar, destinatário).
///
/// Runtime `sqlx::query` (não macro): a coluna `mandate_id` só existe após a
/// 0666, que o cache `.sqlx`/`dsoc_sqlx` pode ainda não ter — bind em runtime
/// evita depender do cache pra esta query.
///
/// # Errors
/// Propaga o `sqlx::Error`.
pub async fn insert_dispatch(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    topic_id: Uuid,
    threshold: i32,
    contact_email: &str,
    mandate_id: Option<Uuid>,
    created_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r"INSERT INTO forum_dispatch (id, topic_id, threshold, contact_email, mandate_id, created_at)
          VALUES ($1, $2, $3, $4, $5, $6)
          ON CONFLICT (topic_id, threshold, mandate_id) DO NOTHING",
    )
    .bind(id)
    .bind(topic_id)
    .bind(threshold)
    .bind(contact_email)
    .bind(mandate_id)
    .bind(created_at)
    .execute(executor)
    .await?;
    Ok(())
}

/// Insere um ALVO do tópico (B1) — idempotente sob corrida.
///
/// Runtime `sqlx::query` (não macro): `forum_topic_target` é da 0666, ausente do
/// cache `.sqlx`/`dsoc_sqlx`.
///
/// # Errors
/// Propaga o `sqlx::Error` (inclui violação de FK se o mandato sumiu na corrida).
pub async fn insert_topic_target(
    executor: impl sqlx::PgExecutor<'_>,
    topic_id: Uuid,
    mandate_id: Uuid,
    created_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r"INSERT INTO forum_topic_target (topic_id, mandate_id, created_at)
          VALUES ($1, $2, $3)
          ON CONFLICT (topic_id, mandate_id) DO NOTHING",
    )
    .bind(topic_id)
    .bind(mandate_id)
    .bind(created_at)
    .execute(executor)
    .await?;
    Ok(())
}

/// `true` se o mandato existe (validação de alvo na criação de tópico — B1).
///
/// # Errors
/// Propaga o `sqlx::Error`.
pub async fn mandate_exists(
    executor: impl sqlx::PgExecutor<'_>,
    mandate_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let row: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM mandate WHERE id = $1")
        .bind(mandate_id)
        .fetch_optional(executor)
        .await?;
    Ok(row.is_some())
}

/// Alvos de um tópico (B1): `(mandate_id, reachable_email)`, ordenados por
/// criação. `reachable_email` já vem `NULL` quando o `public_email` é o
/// placeholder da plataforma — MESMO filtro do `proposal_delivery` (Tier 0):
/// nunca entregamos num inbox morto nem carimbamos recibo/SLA por ele.
///
/// Vazio = tópico SEM alvo (cai no contato curado da seção). Não-vazio com todos
/// os e-mails `NULL` = alvos existem mas nenhum é alcançável (fica pendente).
///
/// Runtime `sqlx::query_as` (não macro): `forum_topic_target` é da 0666.
///
/// # Errors
/// Propaga o `sqlx::Error`.
pub async fn topic_targets(
    executor: impl sqlx::PgExecutor<'_>,
    topic_id: Uuid,
) -> Result<Vec<(Uuid, Option<String>)>, sqlx::Error> {
    let rows: Vec<(Uuid, Option<String>)> = sqlx::query_as(
        r"SELECT tt.mandate_id,
                 CASE WHEN m.public_email ILIKE '%@parlamento.democracia.social.br'
                      THEN NULL ELSE m.public_email END AS reachable_email
            FROM forum_topic_target tt
            JOIN mandate m ON m.id = tt.mandate_id
           WHERE tt.topic_id = $1
           ORDER BY tt.created_at, tt.mandate_id",
    )
    .bind(topic_id)
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// Nomes públicos dos alvos de um tópico (B1) + alcançabilidade, pra UI dizer
/// A QUEM o placar encaminha ("faltam N pontos pra encaminhar a Dep. Fulana").
/// `reachable=false` = alvo com placeholder (Tier 0) — a UI sinaliza "ainda não
/// conectado" em vez de prometer entrega que não acontece.
///
/// Runtime `sqlx::query_as` (não macro): `forum_topic_target` é da 0666.
///
/// # Errors
/// Propaga o `sqlx::Error`.
pub async fn topic_target_names(
    executor: impl sqlx::PgExecutor<'_>,
    topic_id: Uuid,
) -> Result<Vec<(String, bool)>, sqlx::Error> {
    let rows: Vec<(String, bool)> = sqlx::query_as(
        r"SELECT m.display_name,
                 (m.public_email <> ''
                  AND m.public_email NOT ILIKE '%@parlamento.democracia.social.br') AS reachable
            FROM forum_topic_target tt
            JOIN mandate m ON m.id = tt.mandate_id
           WHERE tt.topic_id = $1
           ORDER BY tt.created_at, tt.mandate_id",
    )
    .bind(topic_id)
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// Nome da seção que fornece o contato institucional efetivo (mesma cadeia do
/// [`effective_contact_email`]): o próprio fórum ou o ancestral mais próximo com
/// `contact_email` curado. `None` = nenhum canal na cadeia (encaminhamento fica
/// pendente). Alimenta o rótulo do patamar na UI.
///
/// # Errors
/// Propaga o `sqlx::Error`.
pub async fn effective_contact_name(
    executor: impl sqlx::PgExecutor<'_>,
    forum_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let r: Option<(String,)> = sqlx::query_as(
        r"WITH RECURSIVE chain AS (
             SELECT id, parent_id, contact_email, name, 0 AS depth FROM forum WHERE id = $1
             UNION ALL
             SELECT f.id, f.parent_id, f.contact_email, f.name, chain.depth + 1
               FROM forum f JOIN chain ON f.id = chain.parent_id
              WHERE chain.depth < 4
           )
           SELECT name FROM chain
           WHERE contact_email IS NOT NULL ORDER BY depth LIMIT 1",
    )
    .bind(forum_id)
    .fetch_optional(executor)
    .await?;
    Ok(r.map(|(name,)| name))
}

/// Lista os recibos de envio de um tópico (transparência pública).
///
/// # Errors
/// Propaga o `sqlx::Error`.
pub async fn list_dispatches(
    executor: impl sqlx::PgExecutor<'_>,
    topic_id: Uuid,
) -> Result<Vec<DispatchRow>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT id, threshold, contact_email, sent_at, created_at
           FROM forum_dispatch WHERE topic_id = $1 ORDER BY threshold"#,
        topic_id,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| DispatchRow {
            id: r.id,
            threshold: r.threshold,
            contact_email: r.contact_email,
            sent_at: r.sent_at,
            created_at: r.created_at,
        })
        .collect())
}

/// Eleitorado do território do fórum (base do patamar proporcional — D3).
///
/// Resolve território → linha da tabela `electorate` (seed TSE, migration 0522),
/// espelhando a política do gateway para mandatos:
/// - `municipal` com uf+município → eleitorado daquele município;
/// - `estadual`/`federal` com uf → total da UF;
/// - `federal` sem uf (ex.: nacional) → total nacional (`'BR'`);
/// - fórum SEM esfera (institucional/comunitário) ou sem match → `None`
///   (o chamador cai no piso).
///
/// `municipio` é comparado por igualdade de texto (mesmo critério do gateway);
/// divergência de grafia simplesmente não casa e cai no piso — fail-safe.
///
/// # Errors
/// Propaga o `sqlx::Error`.
pub async fn forum_territory_voters(
    executor: impl sqlx::PgExecutor<'_>,
    forum_id: Uuid,
) -> Result<Option<i64>, sqlx::Error> {
    let r = sqlx::query!(
        r#"SELECT e.voters AS "voters?"
           FROM forum f
           LEFT JOIN electorate e ON f.esfera IS NOT NULL AND (
                (f.esfera = 'municipal' AND f.uf IS NOT NULL AND f.municipio IS NOT NULL
                    AND e.uf = f.uf AND e.municipio = f.municipio)
             OR (f.esfera IN ('estadual', 'federal') AND f.uf IS NOT NULL
                    AND e.uf = f.uf AND e.municipio IS NULL)
             OR (f.esfera = 'federal' AND f.uf IS NULL
                    AND e.uf = 'BR' AND e.municipio IS NULL)
           )
           WHERE f.id = $1"#,
        forum_id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(r.and_then(|r| r.voters))
}

/// E-mail efetivo do fórum: o próprio ou, quando NULL, o do ancestral mais próximo.
///
/// # Errors
/// Propaga o `sqlx::Error`.
pub async fn effective_contact_email(
    executor: impl sqlx::PgExecutor<'_>,
    forum_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let r = sqlx::query!(
        r#"WITH RECURSIVE chain AS (
             SELECT id, parent_id, contact_email, 0 AS depth FROM forum WHERE id = $1
             UNION ALL
             SELECT f.id, f.parent_id, f.contact_email, chain.depth + 1
               FROM forum f JOIN chain ON f.id = chain.parent_id
              WHERE chain.depth < 4
           )
           SELECT contact_email FROM chain
           WHERE contact_email IS NOT NULL ORDER BY depth LIMIT 1"#,
        forum_id,
    )
    .fetch_optional(executor)
    .await?;
    Ok(r.and_then(|r| r.contact_email))
}

/// Deriva o status de transparência (`plena` | `parcial` | `ausente`) a partir do
/// SINAL REAL de alcançabilidade dos gabinetes, não mais de `probe_status` do
/// catálogo `civic_source` (que ficava desatualizado e mostrava `parcial` errado
/// em câmaras cujos vereadores já têm e-mail real).
///
/// - `plena`: existe ≥1 vereador DESTA câmara com e-mail institucional REAL
///   (alcançável) — gabinetes conectados de verdade. `has_reachable_mandate`.
/// - `parcial`: não há gabinete conectado, mas a câmara mantém um site oficial
///   (`base_url` presente no catálogo).
/// - `ausente`: nem gabinete conectado nem site oficial catalogado.
#[must_use]
fn derive_transparency_status(has_reachable_mandate: bool, base_url: Option<&str>) -> String {
    if has_reachable_mandate {
        return "plena".to_owned();
    }
    if base_url.is_some() {
        return "parcial".to_owned();
    }
    "ausente".to_owned()
}

/// Consulta a transparência da câmara de um município. RUNTIME (`sqlx::query_as`
/// sem macro): cruza o catálogo cívico independente `civic_source` (0662/0669) —
/// fora do schema compile-time-checked dos fóruns — com a tabela `mandate`, então
/// não regeneramos `.sqlx` para esta consulta.
///
/// Casa por `(uf, municipio)` case-insensitive via `upper()` dos dois lados — a
/// mesma comparação usada no resto do código. O sinal `plena` vem do EXISTS de um
/// vereador (`sphere='municipal'` + `house='camara_municipal'`) com e-mail REAL
/// (não o placeholder `@parlamento.democracia.social.br`); `official_url` continua
/// vindo de `civic_source.base_url`. Retorna sempre `Some((status, base_url))`.
///
/// # Errors
/// Propaga o `sqlx::Error`.
pub async fn municipal_transparency(
    executor: impl sqlx::PgExecutor<'_>,
    uf: &str,
    municipio: &str,
) -> Result<Option<(String, Option<String>)>, sqlx::Error> {
    // Uma única passagem: o LEFT JOIN LATERAL traz o site oficial (se catalogado)
    // e o EXISTS traz o sinal real de gabinete conectado — sem consumir o executor
    // duas vezes.
    let (base_url, has_reachable): (Option<String>, bool) = sqlx::query_as(
        r#"
        SELECT
            cs.base_url AS base_url,
            EXISTS (
                SELECT 1 FROM mandate
                 WHERE sphere = 'municipal'
                   AND house = 'camara_municipal'
                   AND upper(uf) = upper($1)
                   AND upper(municipio) = upper($2)
                   AND public_email <> ''
                   AND public_email NOT ILIKE '%@parlamento.democracia.social.br'
            ) AS has_reachable
        FROM (SELECT 1) AS d
        LEFT JOIN LATERAL (
            SELECT base_url
              FROM civic_source
             WHERE upper(uf) = upper($1) AND upper(municipio) = upper($2)
             LIMIT 1
        ) cs ON true
        "#,
    )
    .bind(uf)
    .bind(municipio)
    .fetch_one(executor)
    .await?;

    let status = derive_transparency_status(has_reachable, base_url.as_deref());
    Ok(Some((status, base_url)))
}

#[cfg(test)]
mod transparency_tests {
    use super::derive_transparency_status;

    #[test]
    fn plena_when_a_reachable_gabinete_exists() {
        // O sinal `plena` é o gabinete alcançável — independe do site oficial.
        assert_eq!(
            derive_transparency_status(true, Some("https://sapl.x.leg.br")),
            "plena"
        );
        assert_eq!(derive_transparency_status(true, None), "plena");
    }

    #[test]
    fn parcial_when_site_but_no_reachable_gabinete() {
        // Câmara com site oficial, porém nenhum vereador conectado ainda.
        assert_eq!(
            derive_transparency_status(false, Some("https://camara.x")),
            "parcial"
        );
    }

    #[test]
    fn ausente_when_no_gabinete_and_no_site() {
        assert_eq!(derive_transparency_status(false, None), "ausente");
    }
}

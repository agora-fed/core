//! Forum persistence. Every statement is a compile-time-checked `sqlx::query!`
//! (PLAN.md principle 3): no ORM, keyset pagination, UPDATEs guarded by the
//! expected state for idempotency under at-least-once delivery.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// One forum row.
#[derive(Debug, Clone)]
pub struct ForumRow {
    /// Forum id.
    pub id: Uuid,
    /// Owning organization.
    pub org_id: Uuid,
    /// Parent in the hierarchy, when there is one.
    pub parent_id: Option<Uuid>,
    /// Segmento do caminho.
    pub slug: String,
    /// Caminho completo (`sp/santos/saude`).
    pub full_path: String,
    /// Display name.
    pub name: String,
    /// Description.
    pub description: String,
    /// `institucional` | `governanca` | `comunitario`.
    pub kind: String,
    /// Federative sphere, when territorial.
    pub esfera: Option<String>,
    /// State, when territorial.
    pub uf: Option<String>,
    /// Municipality, when territorial.
    pub municipio: Option<String>,
    /// Responsible e-mail (inherited from the parent when NULL).
    pub contact_email: Option<String>,
    /// Forum logo (0543).
    pub avatar_url: Option<String>,
    /// Forum banner (0543).
    pub banner_url: Option<String>,
    /// Dispatch thresholds (countable interactions), ascending.
    pub thresholds: Vec<i32>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// One topic row.
#[derive(Debug, Clone)]
pub struct TopicRow {
    /// Topic id.
    pub id: Uuid,
    /// Owning forum.
    pub forum_id: Uuid,
    /// Author (a local citizen — creation is always local).
    pub author_id: Uuid,
    /// Title.
    pub title: String,
    /// Corpo.
    pub body: String,
    /// Countable interactions (votes + local comments).
    pub interaction_count: i64,
    /// Federated interactions (these never fire a threshold).
    pub federated_interaction_count: i64,
    /// Net favor - contra (the "hot" ordering).
    pub score: i64,
    /// Stances in favour (0544).
    pub favor_count: i64,
    /// Stances against (0544).
    pub contra_count: i64,
    /// Neutral stances (0544).
    pub ponderacao_count: i64,
    /// Total approved comments.
    pub comment_count: i64,
    /// Next threshold to fire (index into `forum.thresholds`).
    pub next_threshold_idx: i32,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// One comment (local or federated).
#[derive(Debug, Clone)]
pub struct CommentRow {
    /// Id.
    pub id: Uuid,
    /// Owning topic.
    pub topic_id: Uuid,
    /// Local author, when local.
    pub author_id: Option<Uuid>,
    /// Remote actor handle, when federated.
    pub remote_handle: Option<String>,
    /// Federado?
    pub federated: bool,
    /// Stance declared with the argument (NULL = no stance / federated).
    pub stance: Option<String>,
    /// Votos a favor DESTE argumento (0545).
    pub favor_count: i64,
    /// Votos contra.
    pub contra_count: i64,
    /// Neutral stances (vestigial, always 0 after ADR-0019).
    pub ponderacao_count: i64,
    /// Karma (SO reputation) of the local author, when local (ADR-0019). `None` = federated/no author.
    pub author_karma: Option<i32>,
    /// Corpo.
    pub body: String,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// A bridging argument (D8.2): the comment plus the cross-side ENDORSEMENTS of
/// the topic. `favor_side`/`contra_side` = how many endorsers of this argument
/// (a `favor` vote on the comment) voted, respectively, `favor`/`contra` ON THE
/// TOPIC. The bridge score is derived by [`crate::domain::bridge_score`].
#[derive(Debug, Clone)]
pub struct BridgeCommentRow {
    /// The comment/argument (local, approved).
    pub comment: CommentRow,
    /// Endorsers whose stance on the topic is `favor`.
    pub favor_side: i64,
    /// Endorsers whose stance on the topic is `contra`.
    pub contra_side: i64,
}

/// One institutional dispatch receipt per threshold.
#[derive(Debug, Clone)]
pub struct DispatchRow {
    /// Id.
    pub id: Uuid,
    /// Patamar cruzado.
    pub threshold: i32,
    /// Destino do envio.
    pub contact_email: String,
    /// When the e-mail went out (NULL = pending in the worker).
    pub sent_at: Option<DateTime<Utc>>,
    /// Creation (the moment of the crossing).
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

/// Find a forum by its full path.
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

/// List the direct children of a forum (or the roots per sphere when `parent_id` is NULL).
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

/// Insert a forum (lazy materialization of the territorial sections).
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

/// Insert a topic.
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

/// Lock a topic `FOR UPDATE` (the TOCTOU window of counters/thresholds).
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

/// List a forum's topics: `hot` (score desc) or most recent (id desc).
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

/// A recent topic with its originating forum (the /f home feed).
#[derive(Debug, Clone)]
pub struct RecentTopicRow {
    /// Topic id.
    pub id: Uuid,
    /// Title.
    pub title: String,
    /// Saldo favor - contra.
    pub score: i64,
    /// Stances in favour.
    pub favor_count: i64,
    /// Stances against.
    pub contra_count: i64,
    /// Neutral stances.
    pub ponderacao_count: i64,
    /// Countable interactions.
    pub interaction_count: i64,
    /// Approved comments.
    pub comment_count: i64,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Forum path (`sp/santos/saude`).
    pub forum_path: String,
    /// Forum name.
    pub forum_name: String,
}

/// Latest topics across ALL forums (home feed), newest first.
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

/// Upsert the citizen's stance (one row per topic-citizen pair; switching
/// stance overwrites — the original first-vote date is preserved).
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

/// Insert a LOCAL comment (approved on creation).
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

/// List a topic's APPROVED comments (oldest first, keyset).
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

/// Find an approved comment (for voting on an argument).
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

/// List a topic's candidate BRIDGING ARGUMENTS (D8.2): approved local comments
/// endorsed (`forum_comment_vote.stance = 'favor'`) by citizens positioned on
/// BOTH sides of the topic. Cross-references each endorser with their topic
/// stance (`forum_topic_vote`) and aggregates per side. The `HAVING` already
/// discards one-sided support — ordering/cutting by bridge score is the caller's
/// job (the formula lives in `domain::bridge_score`, tested).
///
/// Note: only `favor`/`contra` topic stances count; endorsers who voted on the
/// argument but took no topic stance do not enter the bridge tally.
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

/// Upsert the citizen's stance on an ARGUMENT (0545) — one per pair, mutable.
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

/// Recompute an ARGUMENT's counters from its votes (0545).
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

/// Current stance of `citizen`'s vote on `comment_id`, when any (`None` = did not vote).
/// Used to compute the karma delta when a vote changes (ADR-0019).
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

/// Add `delta` to the citizen's karma (SO-style reputation, ADR-0019). May be negative.
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

/// Karma value of a comment vote by stance (SO: favor=+10, contra=−2).
#[must_use]
pub fn karma_value(stance: &str) -> i32 {
    match stance {
        "favor" => 10,
        "contra" => -2,
        _ => 0,
    }
}

/// Recompute the topic's counters from the source tables (under the row lock):
/// stances per side, score = favor - contra; countable interactions = votes +
/// approved local comments + votes on arguments (all local by FK);
/// federated = approved federated comments.
/// Retorna (interactions, federated).
///
/// # Errors
/// Propaga o `sqlx::Error`.
pub async fn refresh_topic_counters(
    executor: impl sqlx::PgExecutor<'_>,
    topic_id: Uuid,
) -> Result<(i64, i64, i64), sqlx::Error> {
    // POINTS scoreboard (ADR-0019, signed): base per voter (±2 if they commented, ±1 if they only
    // voted) + amplification from argument votes (favor↑+2/↓−1, contra↑−2/↓+1). favor_count/contra_count
    // remain the COUNT of votes per side (display); `score` is the signed scoreboard.
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
             -- per-voter base: ±2 if the person ALSO commented (argued), ±1 if they only voted.
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
             -- amplification: vote on the argument × the argument's stance (ADR-0019 matrix).
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

/// Advance the threshold index, **guarded** by the expected previous value (once
/// per threshold even under a race). Returns rows affected (0 = already advanced).
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

/// Record the dispatch receipt of a threshold. `mandate_id` discriminates the
/// recipient (B1): `None` = the section's curated contact (current behaviour);
/// `Some` = one target office — a topic directed at N offices records N receipts
/// on the same threshold. The UNIQUE `(topic_id, threshold, mandate_id)`
/// (NULLS NOT DISTINCT, migration 0666) guarantees once per (threshold, recipient).
///
/// Runtime `sqlx::query` (not the macro): the `mandate_id` column only exists after
/// 0666, which the `.sqlx`/`dsoc_sqlx` cache may not carry yet — binding at runtime
/// avoids depending on the cache for this query.
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

/// Insert a topic TARGET (B1) — idempotent under a race.
///
/// Runtime `sqlx::query` (not the macro): `forum_topic_target` comes from 0666, absent
/// from the `.sqlx`/`dsoc_sqlx` cache.
///
/// # Errors
/// Propagates the `sqlx::Error` (including an FK violation if the mandate vanished in the race).
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

/// `true` when the mandate exists (target validation on topic creation — B1).
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

/// A topic's targets (B1): `(mandate_id, reachable_email)`, ordered by creation.
/// `reachable_email` already arrives `NULL` when `public_email` is the platform
/// placeholder — the SAME filter as `proposal_delivery` (Tier 0): we never deliver
/// to a dead inbox nor stamp a receipt/SLA for one.
///
/// Empty = topic with NO target (falls back to the section's curated contact). Non-empty
/// with every e-mail `NULL` = targets exist but none is reachable (stays pending).
///
/// Runtime `sqlx::query_as` (not the macro): `forum_topic_target` comes from 0666.
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

/// Public names of a topic's targets (B1) + reachability, so the UI can say WHOM
/// the scoreboard dispatches to ("N points left to reach Dep. Fulana").
/// `reachable=false` = a target with a placeholder (Tier 0) — the UI signals "not
/// connected yet" instead of promising a delivery that never happens.
///
/// Runtime `sqlx::query_as` (not the macro): `forum_topic_target` comes from 0666.
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

/// Name of the section providing the effective institutional contact (same chain as
/// [`effective_contact_email`]): the forum itself or the nearest ancestor with a
/// curated `contact_email`. `None` = no channel in the chain (dispatch stays
/// pending). Feeds the threshold label in the UI.
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

/// List a topic's dispatch receipts (public transparency).
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

/// Electorate of the forum's territory (the base of the proportional threshold — D3).
///
/// Resolves territory → a row of the `electorate` table (TSE seed, migration 0522),
/// mirroring the gateway's policy for mandates:
/// - `municipal` with uf+municipality → that municipality's electorate;
/// - `estadual`/`federal` with uf → the UF total;
/// - `federal` without uf (e.g. national) → the national total (`'BR'`);
/// - a forum WITHOUT a sphere (institutional/community) or with no match → `None`
///   (the caller falls back to the floor).
///
/// `municipio` is compared by text equality (the gateway's own criterion);
/// a spelling divergence simply does not match and falls to the floor — fail-safe.
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

/// The forum's effective e-mail: its own or, when NULL, the nearest ancestor's.
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

/// Derive the transparency status (`plena` | `parcial` | `ausente`) from the REAL
/// reachability signal of the offices, no longer from the `civic_source` catalog's
/// `probe_status` (which went stale and wrongly showed `parcial` for councils whose
/// members already have real e-mail).
///
/// - `plena`: at least one council member of THIS council has a REAL institutional
///   e-mail (reachable) — genuinely connected offices. `has_reachable_mandate`.
/// - `parcial`: no connected office, but the council keeps an official site
///   (`base_url` present in the catalog).
/// - `ausente`: neither a connected office nor a catalogued official site.
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

/// Query a municipality's council transparency. RUNTIME (`sqlx::query_as` without
/// the macro): it cross-references the independent civic catalog `civic_source`
/// (0662/0669) — outside the forums' compile-time-checked schema — with the
/// `mandate` table, so we do not regenerate `.sqlx` for this query.
///
/// Matches on `(uf, municipio)` case-insensitively via `upper()` on both sides — the
/// same comparison used elsewhere. The `plena` signal comes from an EXISTS over a
/// council member (`sphere='municipal'` + `house='camara_municipal'`) with a REAL
/// e-mail (not the `@parlamento.democracia.social.br` placeholder); `official_url`
/// still comes from `civic_source.base_url`. Always returns `Some((status, base_url))`.
///
/// # Errors
/// Propaga o `sqlx::Error`.
pub async fn municipal_transparency(
    executor: impl sqlx::PgExecutor<'_>,
    uf: &str,
    municipio: &str,
) -> Result<Option<(String, Option<String>)>, sqlx::Error> {
    // A single pass: the LEFT JOIN LATERAL brings the official site (when catalogued)
    // and the EXISTS brings the real connected-office signal — without consuming the executor
    // twice.
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
        // The `plena` signal IS the reachable office — independent of the official site.
        assert_eq!(
            derive_transparency_status(true, Some("https://sapl.x.leg.br")),
            "plena"
        );
        assert_eq!(derive_transparency_status(true, None), "plena");
    }

    #[test]
    fn parcial_when_site_but_no_reachable_gabinete() {
        // A council with an official site but no connected member yet.
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

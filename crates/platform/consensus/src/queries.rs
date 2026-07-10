//! All persistence for `consensus`. Every statement is an explicit, compile-time-checked
//! `sqlx::query!` (PLAN.md principle 3): no ORM, no query builder, no `SELECT *`, keyset pagination.
//!
//! Vectors are bound as the pgvector text literal `[a,b,c]` and cast `::text::vector` in SQL,
//! because the `sqlx` macros do not map the extension `vector` type for bind parameters. Centroids
//! are recomputed server-side with pgvector's `avg()` aggregate so raw vectors never round-trip into
//! Rust.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// A cluster row for reads (centroid deliberately omitted — callers never need the raw vector).
#[derive(Debug, Clone)]
pub struct ClusterRow {
    /// Cluster id.
    pub id: Uuid,
    /// Owning organization.
    pub org_id: Uuid,
    /// Live member count.
    pub size: i32,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

/// The nearest cluster to a candidate embedding within an org.
#[derive(Debug, Clone, Copy)]
pub struct NearestRow {
    /// Closest cluster id.
    pub id: Uuid,
    /// Its cosine distance to the candidate.
    pub distance: f64,
}

/// Insert a proposal's embedding. `proposal_id` is UNIQUE, so a duplicate raises a unique violation
/// which the service maps to a conflict (idempotent ingest).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn insert_embedding(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    proposal_id: Uuid,
    embedding_literal: &str,
    direction_signature: &[String],
    text_sample: &str,
    created_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"INSERT INTO consensus_embedding
               (id, proposal_id, embedding, direction_signature, text_sample, created_at)
           VALUES ($1, $2, $3::text::vector, $4, $5, $6)"#,
        id,
        proposal_id,
        embedding_literal,
        direction_signature,
        text_sample,
        created_at,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// Non-empty text samples of a cluster's members (NLI pair-judge input). A
/// small sample suffices: the judge asks "same ask?", and three members of a
/// coherent cluster answer that as well as fifty would.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn member_texts(
    executor: impl sqlx::PgExecutor<'_>,
    cluster_id: Uuid,
    limit: i64,
) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT e.text_sample
           FROM consensus_embedding e
           JOIN consensus_cluster_member m ON m.proposal_id = e.proposal_id
           WHERE m.cluster_id = $1 AND e.text_sample <> ''
           LIMIT $2"#,
        cluster_id,
        limit,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(|r| r.text_sample).collect())
}

/// Direction signatures of a cluster's members (stance-veto input). Capped:
/// the veto needs a sample, not the census — 50 members of one civic ask are
/// plenty to expose an antagonistic direction.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn member_signatures(
    executor: impl sqlx::PgExecutor<'_>,
    cluster_id: Uuid,
    limit: i64,
) -> Result<Vec<Vec<String>>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT e.direction_signature AS "direction_signature!: Vec<String>"
           FROM consensus_embedding e
           JOIN consensus_cluster_member m ON m.proposal_id = e.proposal_id
           WHERE m.cluster_id = $1
           LIMIT $2"#,
        cluster_id,
        limit,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(|r| r.direction_signature).collect())
}

/// Find the nearest cluster (exact cosine search) for `org_id`, or `None` if the org has none.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn nearest_cluster(
    executor: impl sqlx::PgExecutor<'_>,
    org_id: Uuid,
    embedding_literal: &str,
) -> Result<Option<NearestRow>, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT id, (centroid <=> $2::text::vector) AS "distance!: f64"
           FROM consensus_cluster
           WHERE org_id = $1
           ORDER BY centroid <=> $2::text::vector
           LIMIT 1"#,
        org_id,
        embedding_literal,
    )
    .fetch_optional(executor)
    .await?;
    Ok(row.map(|r| NearestRow {
        id: r.id,
        distance: r.distance,
    }))
}

/// Create a new single-member cluster seeded by `embedding_literal` (centroid = the embedding).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn insert_cluster(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    org_id: Uuid,
    embedding_literal: &str,
    created_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"INSERT INTO consensus_cluster (id, org_id, centroid, size, created_at)
           VALUES ($1, $2, $3::text::vector, 1, $4)"#,
        id,
        org_id,
        embedding_literal,
        created_at,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// Attach a proposal to a cluster, recording the cosine distance at join time.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn insert_member(
    executor: impl sqlx::PgExecutor<'_>,
    cluster_id: Uuid,
    proposal_id: Uuid,
    distance: f32,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"INSERT INTO consensus_cluster_member (cluster_id, proposal_id, distance)
           VALUES ($1, $2, $3)"#,
        cluster_id,
        proposal_id,
        distance,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// Recompute a cluster's centroid (mean of member embeddings) and size after a join. Returns the
/// new size.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn recompute_centroid(
    executor: impl sqlx::PgExecutor<'_>,
    cluster_id: Uuid,
) -> Result<i32, sqlx::Error> {
    let row = sqlx::query!(
        r#"UPDATE consensus_cluster c
           SET centroid = sub.centroid, size = sub.cnt
           FROM (
               SELECT avg(e.embedding) AS centroid, count(*)::int AS cnt
               FROM consensus_cluster_member m
               JOIN consensus_embedding e ON e.proposal_id = m.proposal_id
               WHERE m.cluster_id = $1
           ) sub
           WHERE c.id = $1
           RETURNING c.size AS "size!: i32""#,
        cluster_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(row.size)
}

/// Fetch a single cluster by id.
///
/// # Errors
/// Propagates the underlying `sqlx::Error` (including `RowNotFound`).
pub async fn get_cluster(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
) -> Result<ClusterRow, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT id, org_id, size, created_at
           FROM consensus_cluster
           WHERE id = $1"#,
        id,
    )
    .fetch_one(executor)
    .await?;
    Ok(ClusterRow {
        id: row.id,
        org_id: row.org_id,
        size: row.size,
        created_at: row.created_at,
    })
}

/// List clusters for an org with keyset pagination over the ascending `id` (UUIDv7 = time-ordered).
/// `after` is the last id seen; `None` starts from the beginning.
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn list_clusters(
    executor: impl sqlx::PgExecutor<'_>,
    org_id: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<ClusterRow>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT id, org_id, size, created_at
           FROM consensus_cluster
           WHERE org_id = $1 AND ($2::uuid IS NULL OR id > $2)
           ORDER BY id
           LIMIT $3"#,
        org_id,
        after,
        limit,
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| ClusterRow {
            id: row.id,
            org_id: row.org_id,
            size: row.size,
            created_at: row.created_at,
        })
        .collect())
}

/// Count the clusters owned by an org (for pagination metadata).
///
/// # Errors
/// Propagates the underlying `sqlx::Error`.
pub async fn count_clusters(
    executor: impl sqlx::PgExecutor<'_>,
    org_id: Uuid,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT count(*) AS "count!: i64"
           FROM consensus_cluster
           WHERE org_id = $1"#,
        org_id,
    )
    .fetch_one(executor)
    .await?;
    Ok(row.count)
}

/// Backlog do re-embed (fatia 2a, 0.28.4): propostas embedadas antes do
/// 0518 — `text_sample` vazio marca tanto a era do stub FNV quanto o
/// intervalo 0.27.x sem amostra. Depois do re-embed a amostra fica
/// não-vazia (texto validado é não-vazio), então a row SAI do backlog —
/// critério idempotente por construção.
pub async fn stale_embedding_proposals(
    executor: impl sqlx::PgExecutor<'_>,
    limit: i64,
) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query_scalar!(
        r#"SELECT proposal_id AS "proposal_id!: Uuid"
           FROM consensus_embedding
           WHERE text_sample = ''
           ORDER BY created_at
           LIMIT $1"#,
        limit,
    )
    .fetch_all(executor)
    .await
}

/// Regrava vetor + assinatura de direção + amostra de uma proposta já
/// embedada. `false` = proposta sem embedding (nada a fazer).
pub async fn update_embedding(
    executor: impl sqlx::PgExecutor<'_>,
    proposal_id: Uuid,
    embedding_literal: &str,
    direction_signature: &[String],
    text_sample: &str,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query!(
        r#"UPDATE consensus_embedding
           SET embedding = $2::text::vector,
               direction_signature = $3,
               text_sample = $4
           WHERE proposal_id = $1"#,
        proposal_id,
        embedding_literal,
        direction_signature,
        text_sample,
    )
    .execute(executor)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Cluster onde a proposta vive (edge UNIQUE por proposta).
pub async fn cluster_of_proposal(
    executor: impl sqlx::PgExecutor<'_>,
    proposal_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar!(
        r#"SELECT cluster_id AS "cluster_id!: Uuid"
           FROM consensus_cluster_member
           WHERE proposal_id = $1"#,
        proposal_id,
    )
    .fetch_optional(executor)
    .await
}

/// Remove o edge de membership de uma proposta apagada; devolve o cluster
/// afetado (se havia) pra o caller recomputar ou dissolver.
pub async fn delete_member(
    executor: impl sqlx::PgExecutor<'_>,
    proposal_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar!(
        r#"DELETE FROM consensus_cluster_member
           WHERE proposal_id = $1
           RETURNING cluster_id AS "cluster_id!: Uuid""#,
        proposal_id,
    )
    .fetch_optional(executor)
    .await
}

/// Remove a embedding órfã (proposta apagada — purge de demo/LGPD art. 18).
pub async fn delete_embedding(
    executor: impl sqlx::PgExecutor<'_>,
    proposal_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"DELETE FROM consensus_embedding WHERE proposal_id = $1"#,
        proposal_id,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// Dissolve o cluster se ficou sem membros (senão o recompute do centroide
/// tentaria gravar `avg()` de zero rows = NULL numa coluna NOT NULL).
/// `true` = cluster removido.
pub async fn delete_cluster_if_empty(
    executor: impl sqlx::PgExecutor<'_>,
    cluster_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query!(
        r#"DELETE FROM consensus_cluster c
           WHERE c.id = $1
             AND NOT EXISTS (SELECT 1 FROM consensus_cluster_member m
                              WHERE m.cluster_id = $1)"#,
        cluster_id,
    )
    .execute(executor)
    .await?;
    Ok(res.rows_affected() > 0)
}

-- 0130_consensus_clustering — owned by `dsoc-consensus` (range 0130, see migrations/REGISTRY.md).
-- Semantic clustering & dedupe via pgvector: embed each proposal, group near-duplicates into one
-- consensus signal (Decidim failure #2). Explicit, auditable SQL (PLAN.md principle 3).
--
-- Cross-crate FKs target ONLY core identity tables (org). `proposal_id` is a bare uuid: the
-- `proposals` table is owned by another crate, so we never FK across that boundary (ARCHITECTURE.md
-- section 3). The embedding dimension (384) matches a small sentence-embedding model.

-- One embedding per proposal. Vectors are L2-normalised by the embedder; cosine distance (`<=>`)
-- is used regardless of magnitude.
CREATE TABLE consensus_embedding (
    id           uuid PRIMARY KEY,
    proposal_id  uuid NOT NULL UNIQUE,
    embedding    vector(384) NOT NULL,
    created_at   timestamptz NOT NULL
);

-- A cluster of near-duplicate proposals. `centroid` is the mean of its members' embeddings;
-- `size` is the live member count, denormalised for cheap reads.
CREATE TABLE consensus_cluster (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES org(id),
    centroid    vector(384) NOT NULL,
    size        integer NOT NULL,
    created_at  timestamptz NOT NULL
);
CREATE INDEX consensus_cluster_org_idx ON consensus_cluster (org_id, id);

-- Membership edge: which proposal belongs to which cluster, and how close it was at join time.
-- `proposal_id` is UNIQUE so a proposal lands in exactly one cluster.
CREATE TABLE consensus_cluster_member (
    cluster_id   uuid NOT NULL REFERENCES consensus_cluster(id),
    proposal_id  uuid NOT NULL UNIQUE,
    distance     real NOT NULL,
    PRIMARY KEY (cluster_id, proposal_id)
);
CREATE INDEX consensus_cluster_member_cluster_idx ON consensus_cluster_member (cluster_id);

-- Note on vector indexing: we deliberately do NOT build an ANN index (ivfflat/hnsw) here. Cluster
-- counts per org stay small, and exact nearest-neighbour search keeps the cosine-distance threshold
-- decision deterministic and auditable — essential for politically-sensitive infrastructure.

COMMENT ON TABLE consensus_cluster IS 'Consensus clusters; emits consensus.cluster.formed / consensus.proposal.merged (crates/core/src/events.rs).';

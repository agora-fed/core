-- 0540_forums_core — hierarchical institutional FORUMS (plan v3, 2026-07-25).
--
-- The platform's thesis applied to forums: society deliberates at /f/<path>,
-- LOCAL interactions (citizen votes + comments) cross configurable
-- thresholds and each threshold fires ONE e-mail to the responsible institution
-- (committee, ministry, department) with a public receipt. Federated participation
-- (fediverse) exists and is shown, but counts SEPARATELY and never fires a dispatch.
--
-- Hierarchy: /f/senado/ccj, /f/sp/santos/saude — one parent level suffices
-- (max depth 3 segments). Default territorial sub-forums (7 per
-- state/municipality) are MATERIALIZED ON DEMAND at the first topic — the seed
-- creates only the roots (~5.7k rows), not the ~39k leaves.
--
-- Cross-crate FKs: core only (org, citizen); the rest are intra-file.
-- Group actor keys (federation, phase F4) are generated on demand — the columns
-- nascem NULL.

BEGIN;

CREATE TABLE forum (
    id               uuid PRIMARY KEY,
    org_id           uuid NOT NULL REFERENCES org(id),
    parent_id        uuid REFERENCES forum(id),
    -- Path segment ('ccj', 'santos', 'saude') — [a-z0-9-], unique per parent.
    slug             text NOT NULL CHECK (slug ~ '^[a-z0-9][a-z0-9-]{0,78}$'),
    -- Full path ('senado/ccj', 'sp/santos/saude') — a cache for O(1) lookup.
    full_path        text NOT NULL,
    name             text NOT NULL CHECK (length(btrim(name)) > 0),
    description      text NOT NULL DEFAULT '',
    kind             text NOT NULL CHECK (kind IN ('institucional', 'governanca', 'comunitario')),
    esfera           text CHECK (esfera IN ('federal', 'estadual', 'municipal')),
    uf               text,
    municipio        text,
    -- Responsible e-mail (committee/ombudsman/department). NULL = inherit from the parent;
    -- curated through the admin panel (never seeded without verification).
    contact_email    text,
    -- Dispatch thresholds in COUNTABLE interactions, ascending, editable per forum.
    thresholds       integer[] NOT NULL DEFAULT '{1000,10000,100000}',
    federated        boolean NOT NULL DEFAULT true,
    -- Chaves do ator Group (geradas no primeiro seguidor remoto — fase F4).
    public_key_pem   text,
    private_key_pem  text,
    hidden_at        timestamptz,
    created_by       uuid REFERENCES citizen(id),
    created_at       timestamptz NOT NULL DEFAULT now(),
    UNIQUE (org_id, full_path),
    UNIQUE NULLS NOT DISTINCT (org_id, parent_id, slug)
);
CREATE INDEX forum_parent_idx ON forum (parent_id, slug);
CREATE INDEX forum_esfera_idx ON forum (org_id, esfera, uf, municipio);

CREATE TABLE forum_topic (
    id                uuid PRIMARY KEY,
    forum_id          uuid NOT NULL REFERENCES forum(id),
    author_id         uuid NOT NULL REFERENCES citizen(id),
    title             text NOT NULL CHECK (length(btrim(title)) > 0),
    body              text NOT NULL CHECK (length(btrim(body)) > 0),
    -- COUNTABLE interactions (votes + local comments) — these fire thresholds.
    interaction_count bigint NOT NULL DEFAULT 0,
    -- FEDERATED interactions (fediverse) — displayed, they never fire.
    federated_interaction_count bigint NOT NULL DEFAULT 0,
    -- Sum of the ±1 votes (the "hot" ordering).
    score             bigint NOT NULL DEFAULT 0,
    comment_count     bigint NOT NULL DEFAULT 0,
    -- Index of the NEXT forum.thresholds entry to fire (one dispatch per threshold).
    next_threshold_idx integer NOT NULL DEFAULT 0,
    ap_object_uri     text,
    hidden_at         timestamptz,
    created_at        timestamptz NOT NULL
);
CREATE INDEX forum_topic_forum_idx ON forum_topic (forum_id, id);
CREATE INDEX forum_topic_hot_idx ON forum_topic (forum_id, score DESC, id DESC);

-- A ±1 vote — PK (topic, citizen): one vote per citizen, and the FK to `citizen`
-- makes the rule "only a local user votes" STRUCTURAL (a remote actor does not exist here).
CREATE TABLE forum_topic_vote (
    topic_id    uuid NOT NULL REFERENCES forum_topic(id),
    citizen_id  uuid NOT NULL REFERENCES citizen(id),
    value       smallint NOT NULL CHECK (value IN (-1, 1)),
    created_at  timestamptz NOT NULL,
    PRIMARY KEY (topic_id, citizen_id)
);

-- A local comment (author_id) OR a federated one (remote_actor_url + moderation).
CREATE TABLE forum_topic_comment (
    id                uuid PRIMARY KEY,
    topic_id          uuid NOT NULL REFERENCES forum_topic(id),
    author_id         uuid REFERENCES citizen(id),
    remote_actor_url  text,
    remote_handle     text,
    federated         boolean NOT NULL DEFAULT false,
    -- Moderation applies to federated ones only; local ones are born approved.
    moderation        text NOT NULL DEFAULT 'approved'
                      CHECK (moderation IN ('pending', 'approved', 'rejected')),
    body              text NOT NULL CHECK (length(btrim(body)) > 0),
    created_at        timestamptz NOT NULL,
    CHECK ((federated AND remote_actor_url IS NOT NULL AND author_id IS NULL)
        OR (NOT federated AND author_id IS NOT NULL))
);
CREATE INDEX forum_topic_comment_topic_idx ON forum_topic_comment (topic_id, id);

-- Institutional dispatch per threshold — the UNIQUE guarantees once per threshold; public receipt.
CREATE TABLE forum_dispatch (
    id             uuid PRIMARY KEY,
    topic_id       uuid NOT NULL REFERENCES forum_topic(id),
    threshold      integer NOT NULL,
    contact_email  text NOT NULL,
    sent_at        timestamptz,
    created_at     timestamptz NOT NULL,
    UNIQUE (topic_id, threshold)
);

COMMENT ON TABLE forum IS 'Fóruns institucionais hierárquicos (/f/<caminho>); emite envios por patamar com recibo (0540).';
COMMENT ON TABLE forum_topic IS 'Tópico de fórum: interações contáveis (locais) vs federadas; patamares 1x.';
COMMENT ON TABLE forum_topic_vote IS 'Voto ±1 — FK citizen = só usuário local vota (estrutural).';
COMMENT ON TABLE forum_dispatch IS 'Recibo do envio institucional por patamar cruzado (1x por patamar).';

-- Prod aplica migrations como postgres; o gateway conecta como dsoc.
ALTER TABLE forum OWNER TO dsoc;
ALTER TABLE forum_topic OWNER TO dsoc;
ALTER TABLE forum_topic_vote OWNER TO dsoc;
ALTER TABLE forum_topic_comment OWNER TO dsoc;
ALTER TABLE forum_dispatch OWNER TO dsoc;

COMMIT;

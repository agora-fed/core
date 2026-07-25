-- 0540_forums_core — FÓRUNS institucionais hierárquicos (plano v3, 2026-07-25).
--
-- A tese da plataforma aplicada a fóruns: a sociedade delibera em /f/<caminho>,
-- as interações LOCAIS (votos + comentários de cidadãos) cruzam patamares
-- configuráveis e cada patamar dispara UM e-mail à instituição responsável
-- (comissão, ministério, secretaria) com recibo público. Participação federada
-- (fediverso) existe e aparece, mas conta SEPARADO e nunca dispara envio.
--
-- Hierarquia: /f/senado/ccj, /f/sp/santos/saude — um nível de pai é suficiente
-- (profundidade máx. 3 segmentos). Sub-fóruns territoriais padrão (7 por
-- estado/município) são MATERIALIZADOS SOB DEMANDA no primeiro tópico — o seed
-- cria só as raízes (~5,7k linhas), não as ~39k folhas.
--
-- Cross-crate FKs: apenas core (org, citizen); demais são intra-arquivo.
-- Chaves do ator Group (federação, fase F4) são geradas sob demanda — colunas
-- nascem NULL.

BEGIN;

CREATE TABLE forum (
    id               uuid PRIMARY KEY,
    org_id           uuid NOT NULL REFERENCES org(id),
    parent_id        uuid REFERENCES forum(id),
    -- Segmento do caminho ('ccj', 'santos', 'saude') — [a-z0-9-], único por pai.
    slug             text NOT NULL CHECK (slug ~ '^[a-z0-9][a-z0-9-]{0,78}$'),
    -- Caminho completo ('senado/ccj', 'sp/santos/saude') — cache pra lookup O(1).
    full_path        text NOT NULL,
    name             text NOT NULL CHECK (length(btrim(name)) > 0),
    description      text NOT NULL DEFAULT '',
    kind             text NOT NULL CHECK (kind IN ('institucional', 'governanca', 'comunitario')),
    esfera           text CHECK (esfera IN ('federal', 'estadual', 'municipal')),
    uf               text,
    municipio        text,
    -- E-mail responsável (comissão/ouvidoria/secretaria). NULL = herda do pai;
    -- curadoria via painel admin (nunca semeado sem verificação).
    contact_email    text,
    -- Patamares de envio em interações CONTÁVEIS, ordem crescente, editável por fórum.
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
    -- Interações CONTÁVEIS (votos + comentários locais) — disparam patamares.
    interaction_count bigint NOT NULL DEFAULT 0,
    -- Interações FEDERADAS (fediverso) — exibidas, nunca disparam.
    federated_interaction_count bigint NOT NULL DEFAULT 0,
    -- Soma dos votos ±1 (ordenação "quente").
    score             bigint NOT NULL DEFAULT 0,
    comment_count     bigint NOT NULL DEFAULT 0,
    -- Índice do PRÓXIMO patamar de forum.thresholds a disparar (envio 1x por patamar).
    next_threshold_idx integer NOT NULL DEFAULT 0,
    ap_object_uri     text,
    hidden_at         timestamptz,
    created_at        timestamptz NOT NULL
);
CREATE INDEX forum_topic_forum_idx ON forum_topic (forum_id, id);
CREATE INDEX forum_topic_hot_idx ON forum_topic (forum_id, score DESC, id DESC);

-- Voto ±1 — PK (topic, citizen): um voto por cidadão, e a FK pra `citizen`
-- torna ESTRUTURAL a regra "só usuário local vota" (ator remoto não existe aqui).
CREATE TABLE forum_topic_vote (
    topic_id    uuid NOT NULL REFERENCES forum_topic(id),
    citizen_id  uuid NOT NULL REFERENCES citizen(id),
    value       smallint NOT NULL CHECK (value IN (-1, 1)),
    created_at  timestamptz NOT NULL,
    PRIMARY KEY (topic_id, citizen_id)
);

-- Comentário local (author_id) OU federado (remote_actor_url + moderação).
CREATE TABLE forum_topic_comment (
    id                uuid PRIMARY KEY,
    topic_id          uuid NOT NULL REFERENCES forum_topic(id),
    author_id         uuid REFERENCES citizen(id),
    remote_actor_url  text,
    remote_handle     text,
    federated         boolean NOT NULL DEFAULT false,
    -- Moderação só se aplica a federados; locais nascem aprovados.
    moderation        text NOT NULL DEFAULT 'approved'
                      CHECK (moderation IN ('pending', 'approved', 'rejected')),
    body              text NOT NULL CHECK (length(btrim(body)) > 0),
    created_at        timestamptz NOT NULL,
    CHECK ((federated AND remote_actor_url IS NOT NULL AND author_id IS NULL)
        OR (NOT federated AND author_id IS NOT NULL))
);
CREATE INDEX forum_topic_comment_topic_idx ON forum_topic_comment (topic_id, id);

-- Envio institucional por patamar — UNIQUE garante 1x por patamar; recibo público.
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

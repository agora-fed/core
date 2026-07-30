-- 0670_socrates_mirror — SOCRATES espelha Ideias Legislativas do e-Cidadania (Senado).
--
-- Tese: no portal e-Cidadania o cidadão SÓ pode apoiar uma Ideia Legislativa —
-- não há voto contra nem argumento. O SOCRATES (bot institucional da
-- plataforma) espelha a ideia como tópico do fórum `senado` pra abrir o debate
-- completo (favor × contra + afirmação-ponte). MVP admin-curado: o admin cola a
-- URL/ID no painel e o gateway busca título + cria o tópico assinado pelo bot.
--
-- `socrates_mirror` deduplica por `ideia_id` (UNIQUE): cada ideia do Senado é
-- espelhada NO MÁXIMO uma vez, e a linha guarda o vínculo com o tópico criado.
--
-- O cidadão-bot tem UUID FIXO (50c7a7e5-…-0001) pra ser referenciável no código
-- sem lookup, `oidc_subject` sintético 'system:socrates' (nunca emitido por
-- IdP — não há credencial de login) e `is_public = true` (perfil visível;
-- ADR-0010: a federação só materializa Actor de perfil público).
--
-- OWNER: ALTER TABLE socrates_mirror OWNER TO dsoc
--
-- Idempotente: rerun-safe (IF NOT EXISTS / ON CONFLICT DO NOTHING).

BEGIN;

CREATE TABLE IF NOT EXISTS socrates_mirror (
    id         uuid PRIMARY KEY,
    -- Id numérico da ideia no e-Cidadania (o `?id=NNNNNN` da URL) — chave de dedup.
    ideia_id   text NOT NULL UNIQUE,
    -- URL canônica da ideia no portal do Senado (atribuição no corpo do tópico).
    source_url text NOT NULL,
    -- O tópico criado no fórum `senado` em nome do SOCRATES.
    topic_id   uuid NOT NULL REFERENCES forum_topic(id),
    created_at timestamptz NOT NULL DEFAULT now()
);

ALTER TABLE socrates_mirror OWNER TO dsoc;

-- Cidadão-bot SOCRATES: autor institucional dos tópicos espelhados. Sem
-- credencial (auth_credential/auth_session nunca apontam pra ele); nível
-- 'email' satisfaz o piso de verificação das superfícies de leitura.
INSERT INTO citizen (
    id, org_id, oidc_subject, verification_level,
    display_name, bio, is_public, created_at
)
VALUES (
    '50c7a7e5-0000-4000-8000-000000000001',
    '11111111-1111-1111-1111-111111111111',
    'system:socrates',
    'email',
    'SOCRATES',
    'Agente cívico da plataforma. Espelha conteúdo público institucional (ex.: Ideias Legislativas do e-Cidadania/Senado) para abrir o debate completo.',
    true,
    now()
)
ON CONFLICT DO NOTHING;

COMMIT;

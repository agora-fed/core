-- 0670_socrates_mirror — SOCRATES espelha Ideias Legislativas do e-Cidadania (Senado).
--
-- Thesis: on the e-Cidadania portal a citizen can ONLY support a Legislative Idea —
-- there is no vote against and no argument. SOCRATES (the platform's institutional
-- bot) mirrors the idea as a topic of the `senado` forum to open the full
-- debate (for × against + bridging claims). An admin-curated MVP: the admin pastes the
-- URL/ID into the panel and the gateway fetches the title + creates the bot-signed topic.
--
-- `socrates_mirror` deduplicates by `ideia_id` (UNIQUE): each Senate idea is
-- mirrored AT MOST once, and the row keeps the link to the created topic.
--
-- The citizen-bot has a FIXED UUID (50c7a7e5-…-0001) so it is referenceable in code
-- without a lookup, a synthetic `oidc_subject` 'system:socrates' (never issued by an
-- IdP — there is no login credential) and `is_public = true` (a visible profile;
-- ADR-0010: federation only materializes an Actor for a public profile).
--
-- OWNER: ALTER TABLE socrates_mirror OWNER TO dsoc
--
-- Idempotente: rerun-safe (IF NOT EXISTS / ON CONFLICT DO NOTHING).

BEGIN;

CREATE TABLE IF NOT EXISTS socrates_mirror (
    id         uuid PRIMARY KEY,
    -- Numeric id of the idea on e-Cidadania (the URL's `?id=NNNNNN`) — the dedup key.
    ideia_id   text NOT NULL UNIQUE,
    -- Canonical URL of the idea on the Senate portal (attribution in the topic body).
    source_url text NOT NULL,
    -- The topic created in the `senado` forum on SOCRATES' behalf.
    topic_id   uuid NOT NULL REFERENCES forum_topic(id),
    created_at timestamptz NOT NULL DEFAULT now()
);

ALTER TABLE socrates_mirror OWNER TO dsoc;

-- The SOCRATES citizen-bot: institutional author of the mirrored topics. It has no
-- credential (auth_credential/auth_session never point at it); the 'email'
-- level satisfies the verification floor of the read surfaces.
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

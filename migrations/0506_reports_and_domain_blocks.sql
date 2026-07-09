-- 0506_reports_and_domain_blocks.sql
--
-- Duas peças que fecham o menu-de-post estilo Mastodon:
--
-- 1. `note_report` — denúncias submetidas por cidadãos sobre publicações
--    (nossas OU do fediverso). Vira fila de trabalho pra moderação humana.
--    Categoria segue o vocabulário do Mastodon (spam / violation / other)
--    pra ser fácil de reciclar numa exportação futura.
-- 2. `domain_block` — bloqueio a nível de HOST inteiro (ex.: bloquear todos
--    de `pravda.social.example`). Escondemos qualquer nota vindo desse host
--    do feed do cidadão. Isso é diferente de `actor_block`, que é por conta.

CREATE TABLE note_report (
    id                 uuid PRIMARY KEY,
    -- Quem denuncia. Sem CASCADE porque queremos o registro histórico
    -- mesmo que a conta suma (auditoria de moderação).
    reporter_id        uuid NOT NULL REFERENCES citizen(id),
    -- object_uri da nota denunciada. Como as notas remotas não têm FK, deixamos
    -- solto igual a note_bookmark — sobrevive a purge do timeline remoto.
    object_uri         text NOT NULL,
    -- Autor da nota (actor_url) — pra facilitar agregação "quantas denúncias
    -- essa conta acumulou".
    author_actor_url   text NOT NULL,
    -- Categoria fixa da denúncia. Vocabulário compatível Mastodon.
    category           text NOT NULL CHECK (category IN ('spam', 'violation', 'other')),
    -- Texto livre do denunciante (opcional, até 2000 chars).
    reason             text,
    created_at         timestamptz NOT NULL DEFAULT now(),
    -- Moderação humana atualiza. NULL = fila.
    resolved_at        timestamptz,
    resolved_by        uuid REFERENCES citizen(id),
    -- Notas do moderador (não expostas ao denunciante).
    resolution_notes   text,
    -- Um cidadão só denuncia a mesma nota uma vez.
    UNIQUE (reporter_id, object_uri)
);

CREATE INDEX note_report_pending_idx
    ON note_report (created_at DESC)
    WHERE resolved_at IS NULL;
CREATE INDEX note_report_author_idx
    ON note_report (author_actor_url);

COMMENT ON TABLE note_report IS
    '0.26.9 (mastodon-parity fase 2C): fila de denúncias submetidas pelo cidadão.';

ALTER TABLE note_report OWNER TO dsoc;

-- ─────────────────────────────────────────────────────────────
-- Bloqueio de domínio inteiro
-- ─────────────────────────────────────────────────────────────
CREATE TABLE domain_block (
    id            uuid PRIMARY KEY,
    citizen_id    uuid NOT NULL REFERENCES citizen(id) ON DELETE CASCADE,
    -- Host normalizado em lowercase (sem esquema, sem porta). Ex.: "pravda.example".
    domain        text NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    UNIQUE (citizen_id, domain)
);
CREATE INDEX domain_block_citizen_idx
    ON domain_block (citizen_id);

COMMENT ON TABLE domain_block IS
    '0.26.9: cidadão esconde do próprio feed qualquer nota vinda desse host.';

ALTER TABLE domain_block OWNER TO dsoc;

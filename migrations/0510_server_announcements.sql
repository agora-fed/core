-- 0510_server_announcements.sql — anúncios da instância inteira.
--
-- Banner amarelo (ou variante) exibido pra qualquer visitante — antes do
-- header ou como toast persistente. Usado pra downtime programado,
-- mudanças de regras, notícias operacionais. Admin cria/edita, cidadão só lê.
--
-- Padrão Mastodon:
--   * texto Markdown-lite (sanitizado antes do render).
--   * janela de exibição opcional (starts_at .. ends_at). NULL = aberta.
--   * publicado_em: quando saiu do rascunho. Não publicado não aparece.

CREATE TABLE server_announcement (
    id           uuid PRIMARY KEY,
    -- Texto do anúncio (Markdown-lite; front sanitiza).
    body         text NOT NULL,
    -- 'info' | 'warning' | 'critical'. UI muda a cor conforme.
    severity     text NOT NULL DEFAULT 'info' CHECK (severity IN ('info', 'warning', 'critical')),
    starts_at    timestamptz,
    ends_at      timestamptz,
    published_at timestamptz,
    created_at   timestamptz NOT NULL DEFAULT now(),
    created_by   uuid REFERENCES citizen(id)
);

CREATE INDEX server_announcement_published_idx
    ON server_announcement (published_at DESC)
    WHERE published_at IS NOT NULL;

COMMENT ON TABLE server_announcement IS
    '0.26.17: anúncios servidor-wide (banner pra downtime, avisos).';

ALTER TABLE server_announcement OWNER TO dsoc;

-- Dismissals — cada cidadão fecha um anúncio localmente e ele some pra ele.
CREATE TABLE server_announcement_dismissal (
    id                   uuid PRIMARY KEY,
    announcement_id      uuid NOT NULL REFERENCES server_announcement(id) ON DELETE CASCADE,
    citizen_id           uuid NOT NULL REFERENCES citizen(id) ON DELETE CASCADE,
    dismissed_at         timestamptz NOT NULL DEFAULT now(),
    UNIQUE (announcement_id, citizen_id)
);
COMMENT ON TABLE server_announcement_dismissal IS
    '0.26.17: cidadão fechou o banner, some pra ele.';
ALTER TABLE server_announcement_dismissal OWNER TO dsoc;

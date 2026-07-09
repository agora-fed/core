-- 0515_webhooks.sql — webhooks servidor-wide.
--
-- Cada webhook é uma URL que recebe POST com payload JSON quando um
-- evento assinado acontece. Igual ao Mastodon:
--   * report.created — nova denúncia entrou na fila.
--   * account.approved — conta aprovada na revisão manual.
--   * account.suspended — conta suspensa pela moderação.
--
-- Segurança: assinamos a request com HMAC-SHA256 usando `secret` gerado
-- na criação e mostrado UMA vez. Header `X-DemocraciaBR-Signature`.

CREATE TABLE webhook (
    id            uuid PRIMARY KEY,
    url           text NOT NULL,
    -- Lista de eventos que disparam. Ex.: ['report.created'].
    events        text[] NOT NULL,
    -- HMAC secret (base64url, 32 bytes). NÃO expõe após criação.
    secret        text NOT NULL,
    enabled       boolean NOT NULL DEFAULT true,
    -- Última tentativa de entrega (status, quando).
    last_status   integer,
    last_delivery_at timestamptz,
    created_at    timestamptz NOT NULL DEFAULT now(),
    created_by    uuid REFERENCES citizen(id)
);
CREATE INDEX webhook_enabled_events_idx ON webhook (enabled)
    WHERE enabled = true;

COMMENT ON TABLE webhook IS
    '0.26.22: webhook servidor-wide. Assinado HMAC-SHA256.';
ALTER TABLE webhook OWNER TO dsoc;

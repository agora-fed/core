-- 0515_webhooks.sql — server-wide webhooks.
--
-- Each webhook is a URL that receives a POST with a JSON payload when a
-- subscribed event happens. Same as Mastodon:
--   * report.created — a new report entered the queue.
--   * account.approved — account approved in manual review.
--   * account.suspended — account suspended by moderation.
--
-- Security: we sign the request with HMAC-SHA256 using `secret`, generated
-- on creation and shown ONCE. Header `X-DemocraciaBR-Signature`.

CREATE TABLE webhook (
    id            uuid PRIMARY KEY,
    url           text NOT NULL,
    -- Events that fire it. E.g. ['report.created'].
    events        text[] NOT NULL,
    -- HMAC secret (base64url, 32 bytes). NOT exposed after creation.
    secret        text NOT NULL,
    enabled       boolean NOT NULL DEFAULT true,
    -- Last delivery attempt (status, when).
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

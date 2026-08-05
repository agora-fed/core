-- 0508_server_domain_blocks.sql — instance-level domain blocks.
--
-- Unlike `domain_block` (per citizen, migration 0506), this table is
-- policy for the whole instance. One row here affects EVERY account on the
-- server: inbound activities from the domain are rejected, outbound deliveries
-- to it are suppressed, and posts that already arrived vanish from the public feed.
--
-- Two severities in Mastodon's vocabulary:
--   * silence  — the domain's posts become invisible to the local public feed
--                (but existing followers keep seeing them); discovery and trends
--                ignoram.
--   * suspend  — corte total. Bloqueia inbox, drop outbound, some do feed
--                for every citizen regardless of whether they already follow.

CREATE TABLE server_domain_block (
    id           uuid PRIMARY KEY,
    -- Host normalized to lowercase, no scheme and no port. E.g. 'pravda.example'.
    domain       text NOT NULL UNIQUE,
    severity     text NOT NULL CHECK (severity IN ('silence', 'suspend')),
    reason       text,
    created_at   timestamptz NOT NULL DEFAULT now(),
    created_by   uuid REFERENCES citizen(id)
);

CREATE INDEX server_domain_block_severity_idx
    ON server_domain_block (severity);

COMMENT ON TABLE server_domain_block IS
    '0.26.12: política da instância — bloqueios de domínio server-wide.';

ALTER TABLE server_domain_block OWNER TO dsoc;

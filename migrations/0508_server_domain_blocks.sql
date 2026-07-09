-- 0508_server_domain_blocks.sql — bloqueios de domínio a nível de instância.
--
-- Diferente de `domain_block` (por-cidadão, migration 0506), esta tabela é
-- política da instância inteira. Uma linha aqui afeta TODAS as contas do
-- servidor: inbound activities do domínio são rejeitadas, outbound entregas
-- pra ele são suprimidas, e posts que já chegaram somem do feed público.
--
-- Duas severidades no vocabulário Mastodon:
--   * silence  — posts do domínio ficam invisíveis pro feed público local
--                (mas quem JÁ segue continua vendo); o discovery e trends
--                ignoram.
--   * suspend  — corte total. Bloqueia inbox, drop outbound, some do feed
--                pra todos os cidadãos independente de já seguir.

CREATE TABLE server_domain_block (
    id           uuid PRIMARY KEY,
    -- Host normalizado em lowercase, sem esquema nem porta. Ex.: 'pravda.example'.
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

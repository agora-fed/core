-- 0539_instance_actor_key — the INSTANCE ACTOR's key (signed fetch).
--
-- Mastodon instances in secure mode (AUTHORIZED_FETCH) require even the GET
-- of an Actor to carry an HTTP Signature. Without it, federated lookup
-- (`/federation/lookup`) and inbound signature verification fail with 401
-- against those instances (real case: wetdry.world).
--
-- This is the instance actor's key pair (`/actors/instance`, type
-- Application — same pattern as Mastodon), generated ONCE by the gateway at boot
-- (lazily; ON CONFLICT DO NOTHING settles the race between replicas). Single row.

BEGIN;

CREATE TABLE IF NOT EXISTS federation_instance_key (
    id               smallint PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    public_key_pem   text NOT NULL,
    private_key_pem  text NOT NULL,
    created_at       timestamptz NOT NULL DEFAULT now()
);

COMMENT ON TABLE federation_instance_key IS
    'Par de chaves do ator de instância (/actors/instance) — assina fetches ActivityPub (0539).';

-- Prod applies migrations as postgres; the gateway connects as dsoc.
ALTER TABLE federation_instance_key OWNER TO dsoc;

COMMIT;

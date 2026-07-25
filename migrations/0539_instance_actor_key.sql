-- 0539_instance_actor_key — chave do ATOR DE INSTÂNCIA (fetch assinado).
--
-- Instâncias Mastodon em modo seguro (AUTHORIZED_FETCH) exigem que até o GET
-- de um Actor venha assinado por HTTP Signature. Sem isso o lookup federado
-- (`/federation/lookup`) e a verificação de assinaturas inbound falham com 401
-- contra essas instâncias (caso real: wetdry.world).
--
-- Este é o par de chaves do ator de instância (`/actors/instance`, tipo
-- Application — mesmo padrão do Mastodon), gerado UMA vez pelo gateway no boot
-- (lazy; ON CONFLICT DO NOTHING resolve corrida entre réplicas). Linha única.

BEGIN;

CREATE TABLE IF NOT EXISTS federation_instance_key (
    id               smallint PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    public_key_pem   text NOT NULL,
    private_key_pem  text NOT NULL,
    created_at       timestamptz NOT NULL DEFAULT now()
);

COMMENT ON TABLE federation_instance_key IS
    'Par de chaves do ator de instância (/actors/instance) — assina fetches ActivityPub (0539).';

-- Prod aplica migrations como postgres; o gateway conecta como dsoc.
ALTER TABLE federation_instance_key OWNER TO dsoc;

COMMIT;

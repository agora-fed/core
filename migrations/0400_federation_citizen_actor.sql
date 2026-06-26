-- 0400_federation_citizen_actor — per-citizen ActivityPub identity (ADR-0005 + ADR-0010 W2).
--
-- Range 0400 is reserved for `federation` in migrations/REGISTRY.md. This is the first migration
-- under that range and lays the foundation: one row per citizen who has opted into a public
-- profile (`citizen.is_public = true`), carrying the RSA keypair that signs every outbound
-- ActivityPub delivery on that citizen's behalf.
--
-- The keypair is generated lazily — only when the citizen flips `is_public = true` for the first
-- time — so the encryption work and the storage footprint are paid for exactly by the citizens
-- who actually federate, not by every registered user. Re-flipping back to private leaves the
-- row in place (so re-publishing reuses the same identity URL on remote inboxes); deletion is a
-- separate operator action.
--
-- SECURITY: `private_pem` is a credential. It never leaves the gateway; the only thing that
-- crosses the federation boundary is `public_pem`, embedded in the Actor document. A leaked DB
-- still leaks the private key — protecting the DB is part of the threat model (LGPD + the
-- "sovereign tier" of the platform). Future hardening: pgcrypto column encryption with a KMS-
-- held key.

CREATE TABLE citizen_actor_key (
    -- 1:1 with citizen — a citizen has at most one federation identity at a time.
    citizen_id  uuid PRIMARY KEY REFERENCES citizen(id),
    -- RSA-2048 PKCS#8 PEM (the format ActivityPub Actor `publicKey` documents and HTTP
    -- Signatures both expect). Public is what the rest of the world reads; private signs.
    private_pem text NOT NULL,
    public_pem  text NOT NULL,
    -- Stamped on insert and on a future key rotation. Lets the Actor document advertise a
    -- stable `id` while the actual key changes (the Actor document references the key id
    -- `<actor>/main-key#<created_at>` so caches invalidate naturally).
    created_at  timestamptz NOT NULL,
    rotated_at  timestamptz
);

-- No additional indexes needed: primary key lookup is the only access pattern. The federation
-- crate reads via `WHERE citizen_id = $1`.

COMMENT ON TABLE citizen_actor_key IS
    'ADR-0010 W2: RSA keypair per public citizen for ActivityPub signing. private_pem is a credential.';

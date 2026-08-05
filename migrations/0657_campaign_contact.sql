-- 0657_campaign_contact.sql — the chapter/campaign's own contact base (F4, #61).
--
-- The chapter UPLOADS its own list (it is the controller; legal basis declared at import). It stays
-- ISOLATED per chapter (LGPD: erasable in bulk per chapter). At import we verify/enrich against the
-- central base: if the e-mail matches a citizen we link `matched_citizen_id` and copy the domicile
-- (uf/municipio_ibge). Dedupe by (chapter, e-mail). The raw list belongs to the chapter; sending
-- stays mediated by the platform.
--
-- Idempotent: rerun-safe.

BEGIN;

CREATE TABLE IF NOT EXISTS campaign_contact (
    id                 uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id             uuid NOT NULL REFERENCES org(id),
    directory_id       uuid NOT NULL REFERENCES party_directory(id),
    email              text NOT NULL,          -- normalised (trim + lowercase)
    name               text,
    phone              text,                   -- optional (E.164), for the future SMS channel
    -- Legal basis declared by whoever imports (LGPD art. 7/11).
    legal_basis        text NOT NULL DEFAULT 'consent'
                       CHECK (legal_basis IN ('consent', 'legitimate_interest', 'contract')),
    -- Verification against the central base: citizen matched by e-mail (if any) + enriched domicile.
    matched_citizen_id uuid REFERENCES citizen(id),
    uf                 text,
    municipio_ibge     integer,
    created_at         timestamptz NOT NULL DEFAULT now()
);

-- Dedupe per chapter (case-insensitive) + "my contacts in this chapter".
CREATE UNIQUE INDEX IF NOT EXISTS campaign_contact_directory_email_uq
    ON campaign_contact (directory_id, lower(email));

COMMENT ON TABLE campaign_contact IS
    '0657 (F4/#61): base própria de contatos por diretório (isolada p/ LGPD; verificada contra a base central no import).';

COMMIT;

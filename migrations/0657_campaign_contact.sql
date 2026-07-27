-- 0657_campaign_contact.sql — base própria de contatos do diretório/campanha (F4, #61).
--
-- O diretório SOBE sua própria lista (controlador = ele; base legal declarada no import). Fica
-- ISOLADA por diretório (LGPD: apagável em bloco por diretório). Na importação, verificamos/
-- enriquecemos contra a base central: se o e-mail casa com um cidadão, ligamos `matched_citizen_id`
-- e copiamos o domicílio (uf/municipio_ibge). Dedupe por (diretório, e-mail). A lista crua é do
-- diretório; envios continuam mediados pela plataforma.
--
-- Idempotente: rerun-safe.

BEGIN;

CREATE TABLE IF NOT EXISTS campaign_contact (
    id                 uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id             uuid NOT NULL REFERENCES org(id),
    directory_id       uuid NOT NULL REFERENCES party_directory(id),
    email              text NOT NULL,          -- normalizado (trim + lowercase)
    name               text,
    phone              text,                   -- opcional (E.164), pro canal SMS futuro
    -- Base legal declarada por quem importa (art. 7º/11 LGPD).
    legal_basis        text NOT NULL DEFAULT 'consent'
                       CHECK (legal_basis IN ('consent', 'legitimate_interest', 'contract')),
    -- Verificação contra a base central: cidadão casado por e-mail (se houver) + domicílio enriquecido.
    matched_citizen_id uuid REFERENCES citizen(id),
    uf                 text,
    municipio_ibge     integer,
    created_at         timestamptz NOT NULL DEFAULT now()
);

-- Dedupe por diretório (case-insensitive) + "meus contatos deste diretório".
CREATE UNIQUE INDEX IF NOT EXISTS campaign_contact_directory_email_uq
    ON campaign_contact (directory_id, lower(email));

COMMENT ON TABLE campaign_contact IS
    '0657 (F4/#61): base própria de contatos por diretório (isolada p/ LGPD; verificada contra a base central no import).';

COMMIT;

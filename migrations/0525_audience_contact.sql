-- Migration 0525 — base de contatos / audiência (0.35.0).
--
-- "Queria ter uma base boa primeiro": antes de campanha, AUDIÊNCIA. Esta
-- tabela é a base única de pessoas em potencial — captadas no site (form
-- "receba novidades", consent LGPD) ou importadas de listas existentes
-- (com base legal declarada). O digest mensal (#12) e qualquer envio
-- futuro consomem DAQUI, sempre honrando unsubscribed_at.
--
-- LGPD:
-- - legal_basis registra POR QUE podemos falar com a pessoa:
--   'consent' (ela pediu no site — consented_at marca quando) ou
--   'legitimate_interest' (import justificado — notes deve dizer a origem).
-- - unsubscribe_token permite descadastro de 1 clique sem login; o
--   descadastro NUNCA apaga a linha (proof de opt-out > re-import acidental).
-- - E-mail é único: recadastro/reimport só atualiza, nunca duplica.

BEGIN;

CREATE TABLE audience_contact (
    id                uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    email             text NOT NULL,
    name              text,
    uf                text,
    municipio         text,
    -- Segmento livre pra recorte de envio: cidadao | politico | imprensa |
    -- parceiro | outro (sem CHECK — o vocabulário vai evoluir).
    segment           text NOT NULL DEFAULT 'cidadao',
    -- De onde veio: 'site_form' | 'import:<slug-da-lista>' | 'manual'.
    source            text NOT NULL,
    legal_basis       text NOT NULL
                      CHECK (legal_basis IN ('consent', 'legitimate_interest')),
    consented_at      timestamptz,
    unsubscribed_at   timestamptz,
    -- Token do link de descadastro de 1 clique (sem login).
    unsubscribe_token text NOT NULL DEFAULT encode(gen_random_bytes(16), 'hex'),
    notes             text,
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX audience_contact_email_uq
    ON audience_contact (lower(email));

CREATE INDEX audience_contact_segment_idx
    ON audience_contact (segment)
    WHERE unsubscribed_at IS NULL;

CREATE INDEX audience_contact_token_idx
    ON audience_contact (unsubscribe_token);

COMMENT ON TABLE audience_contact IS
    '0.35.0: base única de contatos/audiência (captação no site + imports com base legal). Envios futuros filtram unsubscribed_at IS NULL.';

ALTER TABLE audience_contact OWNER TO dsoc;

COMMIT;

-- Migration 0525 — contact base / audience (0.35.0).
--
-- "I wanted a good base first": before a campaign, an AUDIENCE. This
-- table is the single base of potential people — captured on the site (a form
-- "receive updates", LGPD consent) or imported from existing lists
-- (with a declared legal basis). The monthly digest (#12) and any send
-- future send draws FROM HERE, always honouring unsubscribed_at.
--
-- LGPD:
-- - legal_basis records WHY we may talk to the person:
--   'consent' (they asked on the site — consented_at records when) or
--   'legitimate_interest' (a justified import — notes must state the origin).
-- - unsubscribe_token enables one-click opt-out without a login; the
--   unsubscribe NEVER deletes the row (proof of opt-out > accidental re-import).
-- - The e-mail is unique: re-signup/re-import only updates, never duplicates.

BEGIN;

CREATE TABLE audience_contact (
    id                uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    email             text NOT NULL,
    name              text,
    uf                text,
    municipio         text,
    -- Free-form segment for send targeting: `cidadao` | `politico` | `imprensa` |
    -- `parceiro` | `outro` (no CHECK — the vocabulary will evolve).
    segment           text NOT NULL DEFAULT 'cidadao',
    -- Where it came from: 'site_form' | 'import:<list-slug>' | 'manual'.
    source            text NOT NULL,
    legal_basis       text NOT NULL
                      CHECK (legal_basis IN ('consent', 'legitimate_interest')),
    consented_at      timestamptz,
    unsubscribed_at   timestamptz,
    -- Token of the one-click opt-out link (no login).
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

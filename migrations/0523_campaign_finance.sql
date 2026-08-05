-- 0523_campaign_finance.sql — the campaign donation/funding service
-- (0.31.0, the /servicos page). The official — a citizen with a mandate
-- binding (mandate_identity_binding) — declares inflows and outflows on a
-- public page with an IMMUTABLE history: an entry is neither edited nor deleted,
-- it is corrected by revoking (revoked_at) and entering again — the same principle
-- as the scorecard. Donations are inflows with an electoral receipt (receipt_ref).
-- FK to `citizen` only (a core identity table — the REGISTRY rule).

CREATE TABLE campaign_finance_entry (
    id             uuid PRIMARY KEY,
    citizen_id     uuid NOT NULL REFERENCES citizen(id),
    kind           text NOT NULL CHECK (kind IN ('entrada', 'saida')),
    -- Origem (entrada: "Doação — pessoa física", "Fundo partidário"…) ou
    -- expense category (outflow: "Printed material", "Boosting"…).
    descricao      text NOT NULL CHECK (length(descricao) BETWEEN 1 AND 200),
    valor_centavos bigint NOT NULL CHECK (valor_centavos > 0),
    occurred_on    date NOT NULL,
    -- Electoral receipt — present ⇒ the entry is a donation.
    receipt_ref    text CHECK (receipt_ref IS NULL OR length(receipt_ref) <= 60),
    -- Short public name of the donor ("Maria S."), never the document number.
    donor_name     text CHECK (donor_name IS NULL OR length(donor_name) <= 120),
    created_at     timestamptz NOT NULL DEFAULT now(),
    revoked_at     timestamptz,
    -- A receipt/donor only makes sense on an inflow.
    CHECK (kind = 'entrada' OR (receipt_ref IS NULL AND donor_name IS NULL))
);

CREATE INDEX campaign_finance_entry_citizen_idx
    ON campaign_finance_entry (citizen_id, occurred_on DESC, created_at DESC);

COMMENT ON TABLE campaign_finance_entry IS
    '0.31: declaração pública de financiamento de campanha — append-only, correção por revogação.';

CREATE TABLE campaign_fundraising_config (
    citizen_id       uuid PRIMARY KEY REFERENCES citizen(id),
    meta_centavos    bigint CHECK (meta_centavos IS NULL OR meta_centavos > 0),
    -- OFFICIAL fundraising channels (electoral law): a campaign account and/or
    -- crowdfunding approved by the electoral authority. The platform only publicizes.
    bank_account     text CHECK (bank_account IS NULL OR length(bank_account) <= 200),
    crowdfunding_url text CHECK (crowdfunding_url IS NULL OR length(crowdfunding_url) <= 300),
    is_published     boolean NOT NULL DEFAULT false,
    updated_at       timestamptz NOT NULL DEFAULT now()
);

COMMENT ON TABLE campaign_fundraising_config IS
    '0.31: configuração da página de arrecadação da candidatura (meta, meios oficiais, publicação).';

ALTER TABLE campaign_finance_entry OWNER TO dsoc;
ALTER TABLE campaign_fundraising_config OWNER TO dsoc;

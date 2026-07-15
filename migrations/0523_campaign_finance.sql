-- 0523_campaign_finance.sql — serviço de doações/financiamento de campanha
-- (0.31.0, página /servicos). O(a) político(a) — cidadão com vínculo de
-- mandato (mandate_identity_binding) — declara entradas e saídas em página
-- pública com histórico IMUTÁVEL: lançamento não se edita nem se apaga,
-- corrige-se revogando (revoked_at) e lançando de novo — mesmo princípio
-- do placar. Doações são entradas com recibo eleitoral (receipt_ref).
-- FK só para `citizen` (tabela de identidade core — regra do REGISTRY).

CREATE TABLE campaign_finance_entry (
    id             uuid PRIMARY KEY,
    citizen_id     uuid NOT NULL REFERENCES citizen(id),
    kind           text NOT NULL CHECK (kind IN ('entrada', 'saida')),
    -- Origem (entrada: "Doação — pessoa física", "Fundo partidário"…) ou
    -- categoria de gasto (saída: "Material gráfico", "Impulsionamento"…).
    descricao      text NOT NULL CHECK (length(descricao) BETWEEN 1 AND 200),
    valor_centavos bigint NOT NULL CHECK (valor_centavos > 0),
    occurred_on    date NOT NULL,
    -- Recibo eleitoral — presente ⇒ o lançamento é uma doação.
    receipt_ref    text CHECK (receipt_ref IS NULL OR length(receipt_ref) <= 60),
    -- Nome público resumido do(a) doador(a) ("Maria S."), nunca CPF.
    donor_name     text CHECK (donor_name IS NULL OR length(donor_name) <= 120),
    created_at     timestamptz NOT NULL DEFAULT now(),
    revoked_at     timestamptz,
    -- Recibo/doador só fazem sentido em entrada.
    CHECK (kind = 'entrada' OR (receipt_ref IS NULL AND donor_name IS NULL))
);

CREATE INDEX campaign_finance_entry_citizen_idx
    ON campaign_finance_entry (citizen_id, occurred_on DESC, created_at DESC);

COMMENT ON TABLE campaign_finance_entry IS
    '0.31: declaração pública de financiamento de campanha — append-only, correção por revogação.';

CREATE TABLE campaign_fundraising_config (
    citizen_id       uuid PRIMARY KEY REFERENCES citizen(id),
    meta_centavos    bigint CHECK (meta_centavos IS NULL OR meta_centavos > 0),
    -- Meios OFICIAIS de arrecadação (lei eleitoral): conta de campanha e/ou
    -- financiamento coletivo homologado pelo TSE. A plataforma só divulga.
    bank_account     text CHECK (bank_account IS NULL OR length(bank_account) <= 200),
    crowdfunding_url text CHECK (crowdfunding_url IS NULL OR length(crowdfunding_url) <= 300),
    is_published     boolean NOT NULL DEFAULT false,
    updated_at       timestamptz NOT NULL DEFAULT now()
);

COMMENT ON TABLE campaign_fundraising_config IS
    '0.31: configuração da página de arrecadação da candidatura (meta, meios oficiais, publicação).';

ALTER TABLE campaign_finance_entry OWNER TO dsoc;
ALTER TABLE campaign_fundraising_config OWNER TO dsoc;

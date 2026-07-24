-- Migration 0527 — grupos de campanha (Fase 2.3).
--
-- Um "grupo de campanha" é o canal proativo campanha→eleitor que faltava: hoje
-- o político só REAGE a demandas com SLA (painel-mandato). Aqui ele CRIA um
-- espaço próprio — o eleitor entra (join), a campanha publica atualizações, e
-- a base de apoiadores fica visível e mobilizável.
--
-- Dono = um mandato (o mesmo vínculo do gate is_politico / painel / campanha).
-- Não reusa `assembly` (0220): aquilo é corpo participativo da org, SEM dono —
-- semânticas diferentes. Tabelas dedicadas, enxutas.

BEGIN;

-- campaign_group — um grupo por mandato (UNIQUE mandate_id). owner_citizen_id
-- guarda quem criou (o político logado no momento).
CREATE TABLE campaign_group (
    id                uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id            uuid NOT NULL REFERENCES org(id),
    mandate_id        uuid NOT NULL REFERENCES mandate(id),
    owner_citizen_id  uuid NOT NULL REFERENCES citizen(id),
    name              text NOT NULL,
    description       text,
    created_at        timestamptz NOT NULL DEFAULT now(),
    -- Um grupo por mandato: o político tem UM espaço de campanha.
    UNIQUE (mandate_id)
);
CREATE INDEX campaign_group_org_idx ON campaign_group (org_id, id);

-- campaign_group_member — o roster de apoiadores. UNIQUE torna o join idempotente.
CREATE TABLE campaign_group_member (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    group_id    uuid NOT NULL REFERENCES campaign_group(id) ON DELETE CASCADE,
    citizen_id  uuid NOT NULL REFERENCES citizen(id),
    joined_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (group_id, citizen_id)
);
CREATE INDEX campaign_group_member_group_idx ON campaign_group_member (group_id, id);
CREATE INDEX campaign_group_member_citizen_idx ON campaign_group_member (citizen_id);

-- campaign_group_post — atualizações publicadas pela campanha (só o dono posta no MVP).
CREATE TABLE campaign_group_post (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    group_id    uuid NOT NULL REFERENCES campaign_group(id) ON DELETE CASCADE,
    body        text NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX campaign_group_post_group_idx ON campaign_group_post (group_id, created_at DESC, id DESC);

COMMENT ON TABLE campaign_group IS
    '0.39.0: espaço de campanha de um mandato — canal proativo campanha→eleitor.';
COMMENT ON TABLE campaign_group_member IS
    '0.39.0: apoiadores que entraram no grupo (join idempotente).';
COMMENT ON TABLE campaign_group_post IS
    '0.39.0: atualizações publicadas pela campanha no grupo.';

-- O pod do gateway conecta como `dsoc` (não postgres) — sem isto, permission denied.
ALTER TABLE campaign_group OWNER TO dsoc;
ALTER TABLE campaign_group_member OWNER TO dsoc;
ALTER TABLE campaign_group_post OWNER TO dsoc;

COMMIT;

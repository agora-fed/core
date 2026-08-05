-- 0663_civic_activity.sql — extracted legislative activity (AGORA #73, ADR-0018).
--
-- Foundation of civic meta-analysis: bills/minutes (matters), authorship and votes, extracted from
-- the SAME SAPL API as #72 and keyed by (uf, municipality) + `civic_source`. Public primary source
-- (public acts). NLP/LLM distillation (next phase) runs ON TOP of these tables, always citing the
-- source. Does NOT replace `parlamentar_activity.rs` (federal/state via their own API) — it covers municipal.
--
-- Idempotent: rerun-safe. Dedupe by (source_base_url, external_id).

BEGIN;

-- Bills AND minutes (in SAPL, minutes are matters with their own type) — the citizen-browsable text.
CREATE TABLE IF NOT EXISTS civic_proposal (
    id                uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    uf                text NOT NULL,
    municipio         text NOT NULL,
    source_base_url   text NOT NULL,               -- e.g. https://sapl.campinas.sp.leg.br
    external_id       text NOT NULL,               -- matter id in SAPL (stable within the instance)
    numero            integer,
    ano               integer,
    tipo              text,                         -- 'Indicação', 'Projeto de Lei', 'Ata'…
    ementa            text,                         -- summary/subject (basis of the distillation)
    data_apresentacao date,
    created_at        timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX IF NOT EXISTS civic_proposal_source_ext_uidx
    ON civic_proposal (source_base_url, external_id);
CREATE INDEX IF NOT EXISTS civic_proposal_uf_muni_ano_idx
    ON civic_proposal (uf, municipio, ano);

-- Authorship: links the bill to its author. `mandate_id` matched when possible (same matching as #72).
CREATE TABLE IF NOT EXISTS civic_proposal_author (
    proposal_id     uuid NOT NULL REFERENCES civic_proposal(id) ON DELETE CASCADE,
    mandate_id      uuid REFERENCES mandate(id),   -- NULL while unmatched
    autor_external_id text,                        -- member id in SAPL
    autor_nome      text NOT NULL,
    primeiro_autor  boolean NOT NULL DEFAULT false,
    PRIMARY KEY (proposal_id, autor_nome)
);
CREATE INDEX IF NOT EXISTS civic_proposal_author_mandate_idx
    ON civic_proposal_author (mandate_id) WHERE mandate_id IS NOT NULL;

-- Votes (order of the day): outcome per matter — the basis of "what they vote on".
CREATE TABLE IF NOT EXISTS civic_vote (
    id                  uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    uf                  text NOT NULL,
    municipio           text NOT NULL,
    source_base_url     text NOT NULL,
    external_id         text NOT NULL,             -- ordemdia id in SAPL
    data_ordem          date,
    resultado           text,                      -- 'Aprovado', 'Rejeitado'…
    tipo_votacao        text,                      -- 'simbolica', 'nominal', 'secreta'
    materia_external_id text,                       -- matter voted on (links to civic_proposal)
    created_at          timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX IF NOT EXISTS civic_vote_source_ext_uidx
    ON civic_vote (source_base_url, external_id);

COMMENT ON TABLE civic_proposal IS
    '0663 (#73/ADR-0018): proposições+atas extraídas por plataforma (base da destilação cívica).';

COMMIT;

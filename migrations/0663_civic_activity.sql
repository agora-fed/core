-- 0663_civic_activity.sql — atividade legislativa extraída (ÁGORA #73, ADR-0018).
--
-- Fundação da meta-análise cívica: proposições/atas (matérias), autoria e votações, extraídas da
-- MESMA API SAPL do #72 e chaveadas por (uf, município) + `civic_source`. Fonte primária pública
-- (atos públicos). A destilação NLP/LLM (fase seguinte) roda SOBRE estas tabelas, sempre citando a
-- fonte. NÃO substitui `parlamentar_activity.rs` (federal/estadual por API própria) — cobre o municipal.
--
-- Idempotente: rerun-safe. Dedupe por (source_base_url, external_id).

BEGIN;

-- Proposições E atas (no SAPL, atas são matérias com tipo próprio) — o texto navegável do cidadão.
CREATE TABLE IF NOT EXISTS civic_proposal (
    id                uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    uf                text NOT NULL,
    municipio         text NOT NULL,
    source_base_url   text NOT NULL,               -- ex.: https://sapl.campinas.sp.leg.br
    external_id       text NOT NULL,               -- id da matéria no SAPL (estável na instância)
    numero            integer,
    ano               integer,
    tipo              text,                         -- 'Indicação', 'Projeto de Lei', 'Ata'…
    ementa            text,                         -- resumo/objeto (base da destilação)
    data_apresentacao date,
    created_at        timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX IF NOT EXISTS civic_proposal_source_ext_uidx
    ON civic_proposal (source_base_url, external_id);
CREATE INDEX IF NOT EXISTS civic_proposal_uf_muni_ano_idx
    ON civic_proposal (uf, municipio, ano);

-- Autoria: liga a proposição ao autor. `mandate_id` casado quando possível (mesmo casamento do #72).
CREATE TABLE IF NOT EXISTS civic_proposal_author (
    proposal_id     uuid NOT NULL REFERENCES civic_proposal(id) ON DELETE CASCADE,
    mandate_id      uuid REFERENCES mandate(id),   -- NULL enquanto não casado
    autor_external_id text,                        -- id do parlamentar no SAPL
    autor_nome      text NOT NULL,
    primeiro_autor  boolean NOT NULL DEFAULT false,
    PRIMARY KEY (proposal_id, autor_nome)
);
CREATE INDEX IF NOT EXISTS civic_proposal_author_mandate_idx
    ON civic_proposal_author (mandate_id) WHERE mandate_id IS NOT NULL;

-- Votações (ordem do dia): resultado por matéria — base do "o que votam".
CREATE TABLE IF NOT EXISTS civic_vote (
    id                  uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    uf                  text NOT NULL,
    municipio           text NOT NULL,
    source_base_url     text NOT NULL,
    external_id         text NOT NULL,             -- id da ordemdia no SAPL
    data_ordem          date,
    resultado           text,                      -- 'Aprovado', 'Rejeitado'…
    tipo_votacao        text,                      -- 'simbolica', 'nominal', 'secreta'
    materia_external_id text,                       -- matéria votada (liga a civic_proposal)
    created_at          timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX IF NOT EXISTS civic_vote_source_ext_uidx
    ON civic_vote (source_base_url, external_id);

COMMENT ON TABLE civic_proposal IS
    '0663 (#73/ADR-0018): proposições+atas extraídas por plataforma (base da destilação cívica).';

COMMIT;

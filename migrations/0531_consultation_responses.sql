-- Migration 0531 — respostas de consulta pública (0.44.0, Fase 3.3).
--
-- O crate consultations tinha consulta + perguntas, mas NENHUM mecanismo de
-- resposta — a "consulta pública" não era respondível. Esta tabela fecha isso:
-- cada cidadão dá UMA resposta por pergunta (concordo/neutro/discordo),
-- editável (upsert). A superfície pública/participativa vive em
-- `crates/gateway/src/consultas_ext.rs` (runtime queries). FKs miram a tabela de
-- perguntas (dono do dado) e `citizen` (identidade central) — cf. REGISTRY.md.

BEGIN;

CREATE TABLE consultation_response (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    question_id  uuid NOT NULL
                 REFERENCES consultations_consultation_question(id) ON DELETE CASCADE,
    citizen_id   uuid NOT NULL REFERENCES citizen(id),
    answer       text NOT NULL CHECK (answer IN ('concordo', 'neutro', 'discordo')),
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    -- Uma resposta por cidadão por pergunta (o upsert atualiza).
    UNIQUE (question_id, citizen_id)
);

-- Agregação por pergunta (contagem por opção) sem full scan.
CREATE INDEX consultation_response_question_idx
    ON consultation_response (question_id, answer);
-- "Minhas respostas" nesta consulta.
CREATE INDEX consultation_response_citizen_idx
    ON consultation_response (citizen_id);

COMMENT ON TABLE consultation_response IS
    '0.44.0: resposta de um cidadão a uma pergunta de consulta (concordo/neutro/discordo).';

-- O pod do gateway conecta como dsoc.
ALTER TABLE consultation_response OWNER TO dsoc;

COMMIT;

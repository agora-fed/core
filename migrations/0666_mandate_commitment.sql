-- 0666_mandate_commitment.sql — Mandato coletivo: compromisso consultivo declarado (D8.1).
--
-- Tese (accountability ≠ poder): um mandato coletivo (Bancada Ativista/SP, Gabinetona) se
-- COMPROMETE PUBLICAMENTE a ouvir a base antes de votar sobre um tema. O compromisso é
-- VOLUNTÁRIO e CONSULTIVO — mandato é indelegável por lei, então o software entrega apenas a
-- TRANSPARÊNCIA do compromisso, NUNCA vinculação jurídica. A UI nunca diz "vinculante".
--
-- Fluxo: (1) o operador declara um compromisso (tema + descrição); (2) opcionalmente abre uma
-- CONSULTA à base (reusa `consultations_consultation` + `_question`, ADR-0014 — respostas/
-- agregação vêm de graça da página /consulta); (3) registra publicamente se SEGUIU ou NÃO o
-- resultado. O placar deixa de ser "acusação" e vira "instrução".
--
-- FKs: `mandate_id` → tabela de identidade central (permitida por REGISTRY.md). `consultation_id`
-- → `consultations_consultation` (crate consultations) — FK intra-plataforma legítima, declarada
-- em scripts/fk-allow.txt (mesmo caso do broadcast→consulta, 0656).
--
-- Idempotente: rerun-safe.

BEGIN;

CREATE TABLE IF NOT EXISTS mandate_commitment (
    id              uuid PRIMARY KEY,
    mandate_id      uuid NOT NULL REFERENCES mandate(id),
    theme           text NOT NULL,
    description     text NOT NULL,
    -- Só existe o tipo consultivo. O CHECK trava qualquer promessa de vinculação jurídica no
    -- próprio schema — nem por bug de código um compromisso "vinculante" é gravável.
    kind            text NOT NULL DEFAULT 'consultivo' CHECK (kind = 'consultivo'),
    -- Consulta ligada à base (reusa o crate consultations). NULL enquanto não abre a consulta.
    consultation_id uuid REFERENCES consultations_consultation(id),
    -- Resultado declarado do compromisso: o mandato SEGUIU a base, NÃO seguiu, ou ainda pendente.
    outcome         text CHECK (outcome IN ('seguiu', 'nao_seguiu', 'pendente')),
    outcome_note    text,
    -- Compromissos são públicos por padrão (a transparência é o produto).
    is_public       boolean NOT NULL DEFAULT true,
    created_at      timestamptz NOT NULL DEFAULT now()
);

-- Listagem pública por mandato (perfil do político), mais recentes primeiro.
CREATE INDEX IF NOT EXISTS mandate_commitment_mandate_idx
    ON mandate_commitment (mandate_id, created_at DESC);

COMMENT ON TABLE mandate_commitment IS
    '0666 (D8.1): compromisso consultivo VOLUNTÁRIO de um mandato coletivo — declara ouvir a base '
    'sobre um tema e publica se seguiu. Nunca vinculante (mandato é indelegável por lei).';
COMMENT ON COLUMN mandate_commitment.kind IS
    'Sempre ''consultivo'' (CHECK trava). Marca no schema que não há promessa de vinculação jurídica.';
COMMENT ON COLUMN mandate_commitment.outcome IS
    'seguiu | nao_seguiu | pendente — resultado público e imutável do compromisso.';

-- OWNER: o pod do gateway conecta como dsoc.
-- OWNER: ALTER TABLE mandate_commitment OWNER TO dsoc
ALTER TABLE mandate_commitment OWNER TO dsoc;

COMMIT;

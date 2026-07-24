-- Migration 0530 — events_log à prova de adulteração (0.42.0).
--
-- O events_log era "append-only por convenção" (comentário na 0001), sem
-- enforcement — um comprometimento de app/admin podia reescrever ou apagar o
-- histórico, exatamente o que o contrato social diz impedir. Aqui:
--   1. DELETE bloqueado (nada nunca apaga eventos — confirmado no código).
--   2. UPDATE só pode mudar `processed_at` (o worker marca entregue); qualquer
--      alteração nos campos imutáveis é rejeitada.
--   3. `row_hash` = sha256 do conteúdo imutável, setado no INSERT — permite
--      DETECTAR modificação silenciosa (recomputar e comparar) mesmo se alguém
--      burlar os triggers via superusuário.
--
-- O hash-chain/Merkle completo (detecção de reordenação/remoção encadeada) é a
-- "aposta do cartório do silêncio" — Fase 5, feature dedicada.

BEGIN;

ALTER TABLE events_log ADD COLUMN row_hash bytea;

-- Backfill ANTES de criar os triggers (o anti-tamper bloquearia esta escrita).
UPDATE events_log SET row_hash = sha256(convert_to(
    id::text || '|' || org_id::text || '|' || topic || '|' ||
    event_type || '|' || payload::text || '|' || occurred_at::text, 'UTF8'));

-- BEFORE INSERT: carimba o hash de conteúdo.
CREATE OR REPLACE FUNCTION events_log_set_hash() RETURNS trigger AS $$
BEGIN
    NEW.row_hash := sha256(convert_to(
        NEW.id::text || '|' || NEW.org_id::text || '|' || NEW.topic || '|' ||
        NEW.event_type || '|' || NEW.payload::text || '|' || NEW.occurred_at::text, 'UTF8'));
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER events_log_hash_bi
    BEFORE INSERT ON events_log
    FOR EACH ROW EXECUTE FUNCTION events_log_set_hash();

-- BEFORE UPDATE: só `processed_at` pode mudar.
CREATE OR REPLACE FUNCTION events_log_block_tamper_update() RETURNS trigger AS $$
BEGIN
    IF NEW.id          IS DISTINCT FROM OLD.id
    OR NEW.org_id      IS DISTINCT FROM OLD.org_id
    OR NEW.topic       IS DISTINCT FROM OLD.topic
    OR NEW.event_type  IS DISTINCT FROM OLD.event_type
    OR NEW.payload::text IS DISTINCT FROM OLD.payload::text
    OR NEW.occurred_at IS DISTINCT FROM OLD.occurred_at
    OR NEW.row_hash    IS DISTINCT FROM OLD.row_hash THEN
        RAISE EXCEPTION 'events_log é append-only: apenas processed_at pode ser atualizado';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER events_log_block_update
    BEFORE UPDATE ON events_log
    FOR EACH ROW EXECUTE FUNCTION events_log_block_tamper_update();

-- BEFORE DELETE: proibido.
CREATE OR REPLACE FUNCTION events_log_block_delete() RETURNS trigger AS $$
BEGIN
    RAISE EXCEPTION 'events_log é append-only: DELETE proibido';
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER events_log_block_delete
    BEFORE DELETE ON events_log
    FOR EACH ROW EXECUTE FUNCTION events_log_block_delete();

COMMENT ON COLUMN events_log.row_hash IS
    '0.42.0: sha256 do conteúdo imutável — integridade tamper-evident.';

COMMIT;

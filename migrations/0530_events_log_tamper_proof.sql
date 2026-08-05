-- Migration 0530 — a tamper-proof events_log (0.42.0).
--
-- events_log was "append-only by convention" (a comment in 0001), with no
-- enforcement — um comprometimento de app/admin podia reescrever ou apagar o
-- history — exactly what the social contract says must be prevented. Here:
--   1. DELETE blocked (nothing ever deletes events — confirmed in the code).
--   2. UPDATE may only change `processed_at` (the worker marks delivery); any
--      change to the immutable fields is rejected.
--   3. `row_hash` = sha256 of the immutable content, set on INSERT — it allows
--      DETECTING a silent modification (recompute and compare) even if someone
--      bypasses the triggers as a superuser.
--
-- The full hash chain/Merkle tree (detecting chained reordering/removal) is
-- "aposta do cartório do silêncio" — Fase 5, feature dedicada.

BEGIN;

ALTER TABLE events_log ADD COLUMN row_hash bytea;

-- Backfill ANTES de criar os triggers (o anti-tamper bloquearia esta escrita).
UPDATE events_log SET row_hash = sha256(convert_to(
    id::text || '|' || org_id::text || '|' || topic || '|' ||
    event_type || '|' || payload::text || '|' || occurred_at::text, 'UTF8'));

-- BEFORE INSERT: stamps the content hash.
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

-- BEFORE UPDATE: only `processed_at` may change.
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

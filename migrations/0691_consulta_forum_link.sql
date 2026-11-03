-- 0691: link consultas to a forum so the AP object is attributed to a Group actor.
--
-- Each consulta is published under a forum's ActivityPub identity
-- (e.g. @propostas@agora.organica.social, type Group), not a personal
-- Person actor. The forum_id column records which forum owns the consulta.

ALTER TABLE consultations_consultation
    ADD COLUMN IF NOT EXISTS forum_id uuid REFERENCES forum(id);

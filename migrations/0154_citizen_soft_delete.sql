-- Migration 0154 — soft delete of citizen (LGPD art. 18 VI).
--
-- Hard-deleting an account breaks cross FKs (proposal author=citizen,
-- sla mandate_binding, votes, comments). LGPD-compliant solution:
--   1. A `deleted_at` timestamp on citizen.
--   2. Endpoints listing citizens filter `WHERE deleted_at IS NULL`.
--   3. Sensitive data (email, cpf) is wiped on delete; the ID + an opaque
--      handle remain to keep historical accountability references.
--
-- LGPD art. 16: data may be retained when necessary for exercising
-- rights in judicial proceedings or to comply with a legal obligation.
-- Public voting/proposal records qualify (public interest in the
-- historical accountability of a mandate).

BEGIN;

ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS deleted_at timestamptz;

CREATE INDEX IF NOT EXISTS citizen_active_idx
    ON citizen (org_id)
    WHERE deleted_at IS NULL;

COMMENT ON COLUMN citizen.deleted_at IS
    '0.26.0-fase-F: soft-delete LGPD. Quando setado, PII é limpa + login barrado.';

COMMIT;

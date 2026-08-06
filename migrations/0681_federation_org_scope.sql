-- 0681_federation_org_scope.sql — org_id on the federation/notes tables (AGORA #14, phase 1).
--
-- The product's heart was single-tenant in the SCHEMA: `federation_follow` (0401),
-- `federation_outbox_entry` (0402), `federation_timeline_entry` (0403) and
-- `note_hashtag` (0405) carry no org at all, so isolation could not be enforced even
-- in principle — there was nothing to filter on. Every other tenant boundary in the
-- codebase rests on a column these four never had.
--
-- This is the FOUNDATION only. Row-Level Security is deliberately NOT enabled here:
-- an RLS policy reads a per-connection setting, and 380 of this codebase's query sites
-- take a connection from the pool per query and return it immediately. A session-level
-- SET would leak the previous borrower's org to the next one — which does not fail
-- loudly, it silently reads the WRONG tenant, worse than having no RLS. Doing it
-- properly needs a per-request connection scope; that decision is tracked separately.
--
-- BACKFILL, and where it is honest vs. where it is a placeholder:
--
--   * federation_follow, federation_outbox_entry — derived from `citizen.org_id`.
--     These have a real owner, so the value is CORRECT, not a guess.
--
--   * federation_timeline_entry, note_hashtag — a remote note has no local owner. It
--     arrived because somebody follows its author, and with several orgs the same note
--     could belong to several of them at once. One column cannot express that; the
--     right model is per-follower fan-out, which is a larger question than this
--     migration. Backfilled to the single existing org and marked NOT NULL so the
--     schema is uniform, but be aware this column is a PLACEHOLDER on these two tables
--     until fan-out is modelled. It is safe today because exactly one org exists.
--
-- Idempotent: rerun-safe.

BEGIN;

-- ---------------------------------------------------------------------------
-- 1. Columns (nullable first, so the backfill can run before NOT NULL).
-- ---------------------------------------------------------------------------
ALTER TABLE federation_follow          ADD COLUMN IF NOT EXISTS org_id uuid REFERENCES org(id);
ALTER TABLE federation_outbox_entry    ADD COLUMN IF NOT EXISTS org_id uuid REFERENCES org(id);
ALTER TABLE federation_timeline_entry  ADD COLUMN IF NOT EXISTS org_id uuid REFERENCES org(id);
ALTER TABLE note_hashtag               ADD COLUMN IF NOT EXISTS org_id uuid REFERENCES org(id);

-- ---------------------------------------------------------------------------
-- 2. Backfill from the real owner where one exists.
-- ---------------------------------------------------------------------------
UPDATE federation_follow f
   SET org_id = c.org_id
  FROM citizen c
 WHERE c.id = f.citizen_id AND f.org_id IS NULL;

UPDATE federation_outbox_entry o
   SET org_id = c.org_id
  FROM citizen c
 WHERE c.id = o.citizen_id AND o.org_id IS NULL;

-- ---------------------------------------------------------------------------
-- 3. Backfill the ownerless tables to the oldest org (see the header note).
--    `ORDER BY created_at` rather than an id literal: no hardcoded UUID, and on a
--    fresh install the first org created is the install's own.
-- ---------------------------------------------------------------------------
UPDATE federation_timeline_entry
   SET org_id = (SELECT id FROM org ORDER BY created_at LIMIT 1)
 WHERE org_id IS NULL;

UPDATE note_hashtag
   SET org_id = (SELECT id FROM org ORDER BY created_at LIMIT 1)
 WHERE org_id IS NULL;

-- ---------------------------------------------------------------------------
-- 4. NOT NULL — the point of the exercise. A future INSERT that forgets the org
--    now fails at write time instead of producing an unattributable row.
--    Guarded so a fresh database with no org yet does not break the chain.
-- ---------------------------------------------------------------------------
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM org) THEN
        ALTER TABLE federation_follow         ALTER COLUMN org_id SET NOT NULL;
        ALTER TABLE federation_outbox_entry   ALTER COLUMN org_id SET NOT NULL;
        ALTER TABLE federation_timeline_entry ALTER COLUMN org_id SET NOT NULL;
        ALTER TABLE note_hashtag              ALTER COLUMN org_id SET NOT NULL;
    END IF;
END $$;

-- ---------------------------------------------------------------------------
-- 5. Indexes matching how the reads actually filter (org first).
-- ---------------------------------------------------------------------------
CREATE INDEX IF NOT EXISTS federation_follow_org_idx
    ON federation_follow (org_id, citizen_id);
CREATE INDEX IF NOT EXISTS federation_outbox_entry_org_idx
    ON federation_outbox_entry (org_id, created_at DESC);
CREATE INDEX IF NOT EXISTS federation_timeline_entry_org_idx
    ON federation_timeline_entry (org_id, published_at DESC);
CREATE INDEX IF NOT EXISTS note_hashtag_org_idx
    ON note_hashtag (org_id, tag_normalized);

COMMENT ON COLUMN federation_timeline_entry.org_id IS
    '0681 (#14): PLACEHOLDER — a remote note has no local owner; correct modelling is per-follower fan-out.';
COMMENT ON COLUMN note_hashtag.org_id IS
    '0681 (#14): PLACEHOLDER — inherits the ambiguity of the note it indexes.';

COMMIT;

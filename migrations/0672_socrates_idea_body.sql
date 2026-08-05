-- 0672_socrates_idea_body — SOCRATES v3: o espelho passa a carregar a IDEIA
-- ENTIRE idea (agenda + support + status), not just the title.
--
-- What was broken: the mirrored topic carried only the idea's title. The
-- citizen arrived at the forum without the proposal itself — there was nothing to debate. And the
-- support count, which the sweep re-synced into `apoiamentos`, stayed
-- INVISIBLE: the topic body was written once at creation and never
-- rewritten, so the database updated and the forum kept showing the number
-- from the mirroring day (when it showed one at all — 6 of the 11 mirrors have `apoiamentos`
-- NULL because they came from the HTML source, which only gives ids).
--
-- The fix uses e-Cidadania's public PER-IDEA JSON endpoint
-- (`restideialegislativa?id=<ID>`), which returns the full description, the
-- support counter as a bare INTEGER and the idea's institutional status.
-- Hence the new columns:
--
--   * `descricao`       — the AGENDA: the proposal's full text, what was missing
--                         from the topic body. Stored so the refresh knows
--                         whether it changed without rewriting the topic needlessly;
--   * `situacao`        — the institutional status ("Convertida em Proposição",
--                         "Aguardando envio à CDH", …), the datum that says whether the
--                         idea is still alive in the Senate;
--   * `apoiamentos_num` — the counter as a NUMBER. The `apoiamentos` column (text)
--                         continua existindo por compatibilidade: ela guarda a
--                         the Senate's formatting ("20.771", with a thousands dot),
--                         which cannot be compared or sorted. The per-idea
--                         endpoint gives the integer, so here it stays an integer;
--   * `body_synced_at`  — when the topic's BODY was last rewritten
--                         with this data. NULL = the topic still has the old
--                         body (title only): that is exactly the criterion the
--                         backfill uses to know who needs filling in.
--
-- OWNER: ALTER TABLE socrates_mirror OWNER TO dsoc
--
-- Idempotente: rerun-safe (ADD COLUMN IF NOT EXISTS).

BEGIN;

ALTER TABLE socrates_mirror ADD COLUMN IF NOT EXISTS descricao       text;
ALTER TABLE socrates_mirror ADD COLUMN IF NOT EXISTS situacao        text;
ALTER TABLE socrates_mirror ADD COLUMN IF NOT EXISTS apoiamentos_num bigint;
ALTER TABLE socrates_mirror ADD COLUMN IF NOT EXISTS body_synced_at  timestamptz;

-- The sweep's refresh prioritizes the most stale ones (NULLS FIRST = mirrors
-- that never had their body filled come first).
CREATE INDEX IF NOT EXISTS socrates_mirror_body_synced_idx
    ON socrates_mirror (body_synced_at NULLS FIRST);

ALTER TABLE socrates_mirror OWNER TO dsoc;

COMMIT;

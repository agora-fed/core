-- 0669 — widen the civic catalogue's platform taxonomy (AGORA #72, ADR-0017).
-- Sampling 450 "unknown" municipalities (classify_unknown.py, 2026-07-30) revealed
-- platforms outside the original CHECK: e-Legis (~9%!), CESPRO, Siscam, Instar, Betha,
-- Fiorilli, Vialink and generic CMSs (WordPress 14%, Joomla, Drupal) plus bespoke sites.
-- OWNER: the table already belongs to dsoc (0662); the constraint follows the table.

BEGIN;

ALTER TABLE civic_source DROP CONSTRAINT civic_source_platform_check;
ALTER TABLE civic_source ADD CONSTRAINT civic_source_platform_check CHECK (
  platform = ANY (ARRAY[
    -- legislative platforms with/queued for an extractor
    'sapl'::text, 'camaraonline'::text, 'elegis'::text, 'cespro'::text,
    'siscam'::text, 'instar'::text, 'betha'::text, 'fiorilli'::text,
    'vialink'::text, 'ipm'::text, 'camarasempapel'::text,
    -- generic CMS (standalone council site; no structured extractor)
    'wordpress'::text, 'joomla'::text, 'drupal'::text, 'proprio-indefinido'::text,
    -- catalogue states
    'other'::text, 'unknown'::text, 'none'::text
  ])
);

COMMIT;

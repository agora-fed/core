-- 0669 — Amplia a taxonomia de plataformas do catálogo cívico (ÁGORA #72, ADR-0017).
-- A amostragem de 450 municípios "unknown" (classify_unknown.py, 2026-07-30) revelou
-- plataformas fora da CHECK original: e-Legis (~9%!), CESPRO, Siscam, Instar, Betha,
-- Fiorilli, Vialink e CMS genéricos (WordPress 14%, Joomla, Drupal) + sites próprios.
-- OWNER: tabela já é do dsoc (0662); constraint acompanha a tabela.

BEGIN;

ALTER TABLE civic_source DROP CONSTRAINT civic_source_platform_check;
ALTER TABLE civic_source ADD CONSTRAINT civic_source_platform_check CHECK (
  platform = ANY (ARRAY[
    -- plataformas legislativas com/na fila de extrator
    'sapl'::text, 'camaraonline'::text, 'elegis'::text, 'cespro'::text,
    'siscam'::text, 'instar'::text, 'betha'::text, 'fiorilli'::text,
    'vialink'::text, 'ipm'::text, 'camarasempapel'::text,
    -- CMS genérico (site avulso da câmara; sem extrator estruturado)
    'wordpress'::text, 'joomla'::text, 'drupal'::text, 'proprio-indefinido'::text,
    -- estados do catálogo
    'other'::text, 'unknown'::text, 'none'::text
  ])
);

COMMIT;

-- confirmed_camaraonline_seed.sql — câmaras camaraonline confirmadas (ÁGORA #72, ADR-0017).
--
-- URLs verificadas por probe ao vivo (2026-07-27). Assinatura: link `camaraonline.org/cm_<slug>`
-- no HTML; listagem em `/vereadores`; detalhe em `/vereador/<id>/<slug>` (template moderno) ou
-- `/vereadores/<id>/biografia` (template legado). Semeia `civic_source` com uma fila de extração
-- pronta — independe da cobertura do fingerprint. Idempotente (ON CONFLICT por uf+município).
--
-- NOTA de cobertura de e-mail:
--   • Santana de Parnaíba (template moderno): e-mail INSTITUCIONAL em texto plano → enriquecível.
--   • Caieiras (template legado): e-mail ofuscado pelo Cloudflare → NÃO decodificamos (anti-scraping,
--     ADR-0017 manda respeitar ToS). Roster (nome/partido/foto) sim; e-mail não → não enriquecível.

BEGIN;

INSERT INTO civic_source (uf, municipio, platform, base_url, probe_status, last_probed_at) VALUES
  ('SP', 'Santana de Parnaíba', 'camaraonline', 'https://www.camarasantanadeparnaiba.sp.gov.br', 'ok', now()),
  ('SP', 'Caieiras',            'camaraonline', 'https://www.camaracaieiras.sp.gov.br',          'ok', now())
ON CONFLICT (uf, upper(municipio)) DO UPDATE SET
  platform = EXCLUDED.platform, base_url = EXCLUDED.base_url,
  probe_status = EXCLUDED.probe_status, last_probed_at = now();

COMMIT;

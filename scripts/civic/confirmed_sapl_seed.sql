-- confirmed_sapl_seed.sql — 25 câmaras SAPL confirmadas (ÁGORA #72, ADR-0017).
--
-- URLs verificadas por probe ao vivo (2026-07-27) nas 7 UFs prioritárias (SP PR SC RS RJ ES MG).
-- Semeia `civic_source` com uma fila de extração pronta — independe da cobertura do fingerprint.
-- Convenção: https://sapl.<slug>.<uf>.leg.br. Idempotente (ON CONFLICT por uf+município).

BEGIN;

INSERT INTO civic_source (uf, municipio, platform, base_url, probe_status, last_probed_at) VALUES
  ('SP', 'Campinas',              'sapl', 'https://sapl.campinas.sp.leg.br',            'ok', now()),
  ('SP', 'Franca',                'sapl', 'https://sapl.franca.sp.leg.br',              'ok', now()),
  ('SP', 'Fernão',                'sapl', 'https://sapl.fernao.sp.leg.br',              'ok', now()),
  ('PR', 'Cascavel',              'sapl', 'https://sapl.cascavel.pr.leg.br',            'ok', now()),
  ('PR', 'Foz do Iguaçu',         'sapl', 'https://sapl.fozdoiguacu.pr.leg.br',         'ok', now()),
  ('PR', 'Apucarana',             'sapl', 'https://sapl.apucarana.pr.leg.br',           'ok', now()),
  ('PR', 'Telêmaco Borba',        'sapl', 'https://sapl.telemacoborba.pr.leg.br',       'ok', now()),
  ('SC', 'São José',              'sapl', 'https://sapl.saojose.sc.leg.br',             'ok', now()),
  ('SC', 'São Bento do Sul',      'sapl', 'https://sapl.saobentodosul.sc.leg.br',       'ok', now()),
  ('SC', 'Canoinhas',             'sapl', 'https://sapl.canoinhas.sc.leg.br',           'ok', now()),
  ('SC', 'Tijucas',               'sapl', 'https://sapl.tijucas.sc.leg.br',             'ok', now()),
  ('SC', 'São Lourenço do Oeste', 'sapl', 'https://sapl.saolourencodooeste.sc.leg.br',  'ok', now()),
  ('RS', 'Pelotas',               'sapl', 'https://sapl.pelotas.rs.leg.br',             'ok', now()),
  ('RS', 'Agudo',                 'sapl', 'https://sapl.agudo.rs.leg.br',               'ok', now()),
  ('RS', 'Erechim',               'sapl', 'https://sapl.erechim.rs.leg.br',             'ok', now()),
  ('RJ', 'Petrópolis',            'sapl', 'https://sapl.petropolis.rj.leg.br',          'ok', now()),
  ('RJ', 'Volta Redonda',         'sapl', 'https://sapl.voltaredonda.rj.leg.br',        'ok', now()),
  ('RJ', 'Macaé',                 'sapl', 'https://sapl.macae.rj.leg.br',               'ok', now()),
  ('RJ', 'Nova Friburgo',         'sapl', 'https://sapl.novafriburgo.rj.leg.br',        'ok', now()),
  ('RJ', 'Barra Mansa',           'sapl', 'https://sapl.barramansa.rj.leg.br',          'ok', now()),
  ('ES', 'Aracruz',               'sapl', 'https://sapl.aracruz.es.leg.br',             'ok', now()),
  ('ES', 'São Mateus',            'sapl', 'https://sapl.saomateus.es.leg.br',           'ok', now()),
  ('MG', 'Divinópolis',           'sapl', 'https://sapl.divinopolis.mg.leg.br',         'ok', now()),
  ('MG', 'Varginha',              'sapl', 'https://sapl.varginha.mg.leg.br',            'ok', now()),
  ('MG', 'Lavras',                'sapl', 'https://sapl.lavras.mg.leg.br',              'ok', now())
ON CONFLICT (uf, upper(municipio)) DO UPDATE SET
  platform = EXCLUDED.platform, base_url = EXCLUDED.base_url,
  probe_status = EXCLUDED.probe_status, last_probed_at = now();

COMMIT;

-- Migration 0536 — complete the party catalogue with the registered parties that were missing.
--
-- The catalogue held 22 parties; assigning an admin to a small party (e.g. PSTU)
-- blew up the FK `party_administrator(org_id, party_sigla) -> party(org_id, sigla)`
-- because the sigla did not exist in `party`. Here we insert the parties registered
-- with the TSE that were missing. Idempotent (ON CONFLICT DO NOTHING). `tse_number`/`logo_url`/
-- `website` are left for a later enrichment pass.

BEGIN;

INSERT INTO party (org_id, sigla, name, created_at, updated_at) VALUES
 ('11111111-1111-1111-1111-111111111111','PSTU','Partido Socialista dos Trabalhadores Unificado',now(),now()),
 ('11111111-1111-1111-1111-111111111111','PCB','Partido Comunista Brasileiro',now(),now()),
 ('11111111-1111-1111-1111-111111111111','PCO','Partido da Causa Operária',now(),now()),
 ('11111111-1111-1111-1111-111111111111','UP','Unidade Popular',now(),now()),
 ('11111111-1111-1111-1111-111111111111','DC','Democracia Cristã',now(),now()),
 ('11111111-1111-1111-1111-111111111111','AGIR','Agir',now(),now()),
 ('11111111-1111-1111-1111-111111111111','MOBILIZA','Mobiliza',now(),now()),
 ('11111111-1111-1111-1111-111111111111','PMB','Partido da Mulher Brasileira',now(),now()),
 ('11111111-1111-1111-1111-111111111111','PRTB','Partido Renovador Trabalhista Brasileiro',now(),now())
ON CONFLICT (org_id, sigla) DO NOTHING;

COMMIT;

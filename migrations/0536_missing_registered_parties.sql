-- Migration 0536 — completa o catálogo de partidos com os registrados que faltavam.
--
-- O catálogo tinha 22 partidos; ao designar admin de um partido pequeno (ex.: PSTU),
-- a FK `party_administrator(org_id, party_sigla) -> party(org_id, sigla)` estourava
-- porque a sigla não existia em `party`. Aqui inserimos os partidos com registro no
-- TSE que faltavam. Idempotente (ON CONFLICT DO NOTHING). `tse_number`/`logo_url`/
-- `website` ficam para um enriquecimento posterior.

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

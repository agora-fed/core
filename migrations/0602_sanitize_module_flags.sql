-- Migration 0602 — saneamento das flags `module.*` pré-existentes (R0.5 / #42, ADR-0011 P3.4).
--
-- O gate de módulo (module_gate.rs) passa a LER `admin_feature_flag` com chave `module.<id>`:
-- uma linha `enabled=false` desliga o módulo pra aquela org. Qualquer linha `module.*` criada
-- ANTES do gate existir (experimento, teste) viraria load-bearing retroativamente e desligaria
-- um módulo silenciosamente. Este saneamento remove essas linhas legadas — a partir daqui, só
-- linhas criadas deliberadamente pelo painel de módulos valem.
--
-- Em prod (2026-07-26) não havia nenhuma; a migração é idempotente e segura mesmo assim.

BEGIN;

DELETE FROM admin_feature_flag WHERE key LIKE 'module.%';

COMMIT;

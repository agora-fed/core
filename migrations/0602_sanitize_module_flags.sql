-- Migration 0602 — sanitise pre-existing `module.*` flags (R0.5 / #42, ADR-0011 P3.4).
--
-- The module gate (module_gate.rs) now READS `admin_feature_flag` with key `module.<id>`:
-- an `enabled=false` row switches the module off for that org. Any `module.*` row created
-- BEFORE the gate existed (an experiment, a test) would retroactively become load-bearing and
-- silently switch a module off. This sanitisation removes those legacy rows — from here on, only
-- rows created deliberately from the modules panel count.
--
-- In prod (2026-07-26) there were none; the migration is idempotent and safe regardless.

BEGIN;

DELETE FROM admin_feature_flag WHERE key LIKE 'module.%';

COMMIT;

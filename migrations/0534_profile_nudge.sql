-- Migration 0534 — convite pra completar o perfil (0.49.0, Fase 4 adesão).
--
-- Cidadãos que se cadastraram mas nunca preencheram o perfil (sem display_name
-- ou handle) recebem, por AÇÃO DO ADMIN, um e-mail convidando a completar. Esta
-- coluna marca quando o convite foi enviado, pra nunca repetir sem o admin
-- mandar de novo. Superfície em `crates/gateway/src/profile_nudge.rs` (runtime).

BEGIN;

ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS profile_nudge_sent_at timestamptz;

COMMENT ON COLUMN citizen.profile_nudge_sent_at IS
    '0.49.0: quando o admin convidou este cidadão a completar o perfil (NULL = nunca).';

COMMIT;

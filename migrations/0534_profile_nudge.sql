-- Migration 0534 — invitation to complete the profile (0.49.0, Phase 4 adoption).
--
-- Citizens who signed up but never filled in their profile (no display_name
-- or handle) receive, BY ADMIN ACTION, an e-mail inviting them to complete it. This
-- column records when the invitation was sent, so it never repeats unless the admin
-- sends it again. Surface in `crates/gateway/src/profile_nudge.rs` (runtime).

BEGIN;

ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS profile_nudge_sent_at timestamptz;

COMMENT ON COLUMN citizen.profile_nudge_sent_at IS
    '0.49.0: quando o admin convidou este cidadão a completar o perfil (NULL = nunca).';

COMMIT;

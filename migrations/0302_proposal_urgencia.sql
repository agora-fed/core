-- Migration 0302 — proposal urgency level (Slice D — 0.25.0-fediverse).
--
-- Marks the proposal as 'comum' (default) or 'urgente'. In the urgent case the
-- vote gate requires `citizen.titulo_status ∈ ('validated','verified')` —
-- separating civic participation (every citizen) from binding decision
-- (a citizen provably eligible to vote in the real Brazil).
--
-- Additive + backward-compatible: existing proposals become 'comum'; no old
-- flow breaks. The front-end UX highlights the 🔥 URGENTE badge.

BEGIN;

ALTER TABLE proposal
    ADD COLUMN IF NOT EXISTS urgencia text NOT NULL DEFAULT 'comum'
        CHECK (urgencia IN ('comum','urgente'));

COMMENT ON COLUMN proposal.urgencia IS
    '0.25.0-fediverso: nível de urgência. urgente ⇒ voto exige titulo_status validated/verified.';

-- Owner alignment (same reason as 0106/0107 — migrations run as postgres in prod).
-- Only ALTER COLUMN here, so the original table ownership is preserved.

COMMIT;

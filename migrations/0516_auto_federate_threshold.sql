-- 0516_auto_federate_threshold.sql
--
-- Phase E complete (server-side auto-federation): when a citizen's proposal
-- crosses the consequence trigger (`ProposalThresholdCrossed`), the worker
-- publishes a public Note on the author's behalf — automatic amplification
-- on the fediverse, without depending on a click in the banner.
--
-- Only those already federable (citizen.is_public + handle) AND who have not
-- switched this preference off. Default true: the public profile is already the
-- federation opt-in (ADR-0010); this is just per-event refinement.

ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS auto_federate_threshold boolean NOT NULL DEFAULT true;

COMMENT ON COLUMN citizen.auto_federate_threshold IS
    '0.26.24: publicar Note pública automática quando a proposta do cidadão cruza o gatilho de consequência. Só tem efeito com is_public = true.';

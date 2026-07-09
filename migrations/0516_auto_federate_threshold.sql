-- 0516_auto_federate_threshold.sql
--
-- Fase E completa (auto-federação server-side): quando a proposta de um
-- cidadão cruza o gatilho de consequência (`ProposalThresholdCrossed`), o
-- worker publica uma Note pública em nome do autor — amplificação automática
-- no fediverso, sem depender do clique no banner.
--
-- Só federa quem já é federável (citizen.is_public + handle) E não desligou
-- esta preferência. Default true: o perfil público já é o opt-in de
-- federação (ADR-0010); aqui é só o refinamento por evento.

ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS auto_federate_threshold boolean NOT NULL DEFAULT true;

COMMENT ON COLUMN citizen.auto_federate_threshold IS
    '0.26.24: publicar Note pública automática quando a proposta do cidadão cruza o gatilho de consequência. Só tem efeito com is_public = true.';

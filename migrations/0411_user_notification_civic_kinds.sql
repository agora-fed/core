-- Migration 0411 — kinds cívicas em `user_notification` (0.25.0-fediverso Feed).
--
-- A tabela nasceu (0406) só com kinds Mastodon (mention/reply/favourite/reblog/follow).
-- Agora que o loop 'propor → cluster → threshold → SLA → resposta OU silêncio' está
-- em produção (dsoc-consequence + dsoc-scorecard), o cidadão precisa saber:
--
-- - `proposal_threshold`: sua proposta cruzou o gatilho da consequência.
-- - `sla_started`: o relógio do mandato começou a rodar sobre sua proposta.
-- - `sla_response`: o mandato respondeu (accountability convertida).
-- - `sla_expired`: o SLA venceu sem resposta — silêncio público registrado.
--
-- Aditivo: CHECK constraint troca, nada de dado migra.

BEGIN;

ALTER TABLE user_notification
    DROP CONSTRAINT user_notification_kind_check;

ALTER TABLE user_notification
    ADD CONSTRAINT user_notification_kind_check
    CHECK (kind IN (
        -- Fediverso (0406).
        'mention', 'reply', 'favourite', 'reblog', 'follow',
        -- Cívicas (0411, 0.25.0-fediverso Feed).
        'proposal_threshold', 'sla_started', 'sla_response', 'sla_expired'
    ));

COMMIT;

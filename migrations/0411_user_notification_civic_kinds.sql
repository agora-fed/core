-- Migration 0411 — civic kinds in `user_notification` (0.25.0-fediverse Feed).
--
-- The table was born (0406) with Mastodon kinds only (mention/reply/favourite/reblog/follow).
-- Now that the 'propose → cluster → threshold → SLA → answer OR silence' loop is
-- in production (dsoc-consequence + dsoc-scorecard), the citizen needs to know:
--
-- - `proposal_threshold`: their proposal crossed the consequence trigger.
-- - `sla_started`: the mandate's clock started running on their proposal.
-- - `sla_response`: the mandate answered (accountability delivered).
-- - `sla_expired`: the SLA expired with no answer — public silence recorded.
--
-- Additive: the CHECK constraint is swapped, no data migrates.

BEGIN;

ALTER TABLE user_notification
    DROP CONSTRAINT user_notification_kind_check;

ALTER TABLE user_notification
    ADD CONSTRAINT user_notification_kind_check
    CHECK (kind IN (
        -- Fediverse (0406).
        'mention', 'reply', 'favourite', 'reblog', 'follow',
        -- Civic (0411, 0.25.0-fediverse Feed).
        'proposal_threshold', 'sla_started', 'sla_response', 'sla_expired'
    ));

COMMIT;

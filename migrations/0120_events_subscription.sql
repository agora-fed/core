-- 0120_events_subscription — owned by `dsoc-events` (range 0120, see migrations/REGISTRY.md).
-- Durable per-subscriber, per-topic cursors over the append-only `events_log` (baseline 0001).
-- Explicit SQL, auditable (PLAN.md principle 3). PostgreSQL 16+.
--
-- The cursor is a keyset position into `events_log` ordered by (occurred_at, id): a subscriber
-- has durably handled every delivery up to and including this point. Polling resumes strictly
-- after it. No cross-crate foreign keys are required (subscribers are logical names; topics are
-- the frozen routing strings from `dsoc_core::events::EventTopic`).

CREATE TABLE events_subscription (
    -- Logical subscriber name (e.g. a consumer/worker identity). Part of the primary key.
    subscriber          text NOT NULL,
    -- Frozen routing topic string (proposals|votes|comments|consensus|consequence|
    -- mandates|scorecard|moderation|auth|notify) — see dsoc_core::events::EventTopic::as_str.
    topic               text NOT NULL,
    -- Keyset cursor into events_log: the (occurred_at, id) of the last durably handled delivery.
    -- NULL/NULL means the subscriber has not consumed anything yet (poll from the beginning).
    cursor_occurred_at  timestamptz,
    cursor_event_id     uuid,
    -- When this cursor was last advanced (written from the injected dsoc_core::Clock).
    updated_at          timestamptz NOT NULL,
    PRIMARY KEY (subscriber, topic),
    -- Either both cursor columns are set, or neither (a cursor is a full keyset position).
    CONSTRAINT events_subscription_cursor_paired
        CHECK ((cursor_occurred_at IS NULL) = (cursor_event_id IS NULL))
);

COMMENT ON TABLE events_subscription IS
    'Per-subscriber, per-topic keyset cursors over events_log; owned by dsoc-events.';

-- 0002_events_consumed — consumer idempotency ledger (ADR-0007). db/core-owned infra.
CREATE TABLE events_consumed (
    consumer    text NOT NULL,
    event_id    uuid NOT NULL,
    consumed_at timestamptz NOT NULL,
    PRIMARY KEY (consumer, event_id)
);

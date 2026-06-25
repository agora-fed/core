-- 0110_notify_fanout — multi-channel fan-out tables. Owned by `dsoc-notify`.
-- Explicit SQL, auditable (PLAN.md principle 3). Cross-crate FKs target ONLY the
-- core identity tables org/citizen (REGISTRY.md). No IPv4 defaults anywhere.

-- ---------------------------------------------------------------------------
-- Outbox: one row per intended notification, fanned out to a single channel.
-- A worker dispatches pending rows and records the outcome as a receipt.
-- ---------------------------------------------------------------------------
CREATE TABLE notify_outbox (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES org(id),
    citizen_id  uuid NOT NULL REFERENCES citizen(id),
    -- Delivery channel; constrained to the supported fan-out targets.
    channel     text NOT NULL CHECK (channel IN ('push', 'email', 'whatsapp')),
    -- Opaque, channel-agnostic message body (rendered per-channel by the sender).
    payload     jsonb NOT NULL,
    -- Lifecycle: pending -> dispatched | failed.
    status      text NOT NULL DEFAULT 'pending'
                CHECK (status IN ('pending', 'dispatched', 'failed')),
    -- Number of send attempts made so far (bounded retry/backoff).
    attempts    integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    created_at  timestamptz NOT NULL
);

-- Scan support for the dispatch worker: oldest pending rows first.
CREATE INDEX notify_outbox_pending_idx
    ON notify_outbox (created_at, id)
    WHERE status = 'pending';

-- ---------------------------------------------------------------------------
-- Delivery receipts: append-only audit trail of every send attempt outcome.
-- ---------------------------------------------------------------------------
CREATE TABLE notify_delivery_receipt (
    id              uuid PRIMARY KEY,
    notification_id uuid NOT NULL REFERENCES notify_outbox(id),
    -- Outcome of this attempt: delivered | failed.
    status          text NOT NULL CHECK (status IN ('delivered', 'failed')),
    -- Non-sensitive diagnostic detail (never secrets).
    detail          text,
    created_at      timestamptz NOT NULL
);

CREATE INDEX notify_delivery_receipt_notification_idx
    ON notify_delivery_receipt (notification_id, created_at);

-- ---------------------------------------------------------------------------
-- Device tokens for push delivery, registered per citizen + platform.
-- ---------------------------------------------------------------------------
CREATE TABLE notify_device_token (
    id          uuid PRIMARY KEY,
    citizen_id  uuid NOT NULL REFERENCES citizen(id),
    -- Push platform the token belongs to.
    platform    text NOT NULL CHECK (platform IN ('ios', 'android', 'web')),
    -- Provider-issued device token (opaque).
    token       text NOT NULL,
    created_at  timestamptz NOT NULL,
    -- A given device token is unique within a citizen+platform.
    UNIQUE (citizen_id, platform, token)
);

CREATE INDEX notify_device_token_citizen_idx
    ON notify_device_token (citizen_id);

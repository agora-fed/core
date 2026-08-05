-- Migration 0111 — Web Push subscriptions (RFC 8291), owned por dsoc-notify.
--
-- notify_device_token (0110) was built for "1 opaque token per device (APNs/FCM)".
-- Web Push RFC 8291 is richer: each client has an endpoint (the push server's URL
-- do navegador) + p256dh + auth (ambos chaves ECDH). Modelar como tabela nova
-- is clearer than trying to squeeze 3 values into one opaque "token".
--
-- UNIQUE (citizen_id, endpoint): a re-subscribe (same browser, expiry of the
-- push server's token → auto renewal) updates p256dh/auth instead of
-- duplicating. The citizen cascade kills the subscription if the account is deleted.

BEGIN;

CREATE TABLE notify_web_push_subscription (
    id           uuid PRIMARY KEY,
    citizen_id   uuid NOT NULL REFERENCES citizen(id) ON DELETE CASCADE,
    -- URL of the push service (fcm.googleapis.com/… for Chrome, mozilla push for Firefox etc.).
    endpoint     text NOT NULL,
    -- base64url do ECDH public key do UA (RFC 8291 §4.1). ~87 chars.
    p256dh       text NOT NULL,
    -- base64url do 16-byte shared secret (auth token). ~22 chars.
    auth         text NOT NULL,
    -- User-Agent at subscription time — it helps show "your Chrome on the laptop"
    -- in the device list under Settings. No sensitive PII.
    user_agent   text,
    created_at   timestamptz NOT NULL,
    -- Marks subs that already failed with 410 Gone (the endpoint expired). We delete them
    -- in the next cleanup; meanwhile we do not try to send again.
    dead_at      timestamptz,

    UNIQUE (citizen_id, endpoint)
);

CREATE INDEX notify_web_push_citizen_alive_idx
    ON notify_web_push_subscription (citizen_id)
    WHERE dead_at IS NULL;

COMMENT ON TABLE notify_web_push_subscription IS
    '0.25.0-fediverso: subscriptions Web Push RFC 8291 por cidadão. Enviar via web-push crate + VAPID_PRIVATE_KEY/VAPID_SUBJECT do env.';

ALTER TABLE notify_web_push_subscription OWNER TO dsoc;

COMMIT;

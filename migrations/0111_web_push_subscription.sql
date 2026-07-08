-- Migration 0111 — Web Push subscriptions (RFC 8291), owned por dsoc-notify.
--
-- notify_device_token (0110) foi feita pra "1 token opaco por device (APNs/FCM)".
-- Web Push RFC 8291 é mais rico: cada cliente tem endpoint (URL do push server
-- do navegador) + p256dh + auth (ambos chaves ECDH). Modelar como tabela nova
-- é mais claro do que tentar espremer 3 valores num "token" opaco.
--
-- UNIQUE (citizen_id, endpoint): re-subscribe (mesmo browser, expiração do
-- token do push server → auto renovação) atualiza p256dh/auth ao invés de
-- duplicar. Cascade de citizen mata subscription se conta for excluída.

BEGIN;

CREATE TABLE notify_web_push_subscription (
    id           uuid PRIMARY KEY,
    citizen_id   uuid NOT NULL REFERENCES citizen(id) ON DELETE CASCADE,
    -- URL do push service (fcm.googleapis.com/… pro Chrome, mozilla push pro Firefox etc.).
    endpoint     text NOT NULL,
    -- base64url do ECDH public key do UA (RFC 8291 §4.1). ~87 chars.
    p256dh       text NOT NULL,
    -- base64url do 16-byte shared secret (auth token). ~22 chars.
    auth         text NOT NULL,
    -- User-Agent no momento da inscrição — ajuda a mostrar "seu Chrome no laptop"
    -- na lista de devices em Configurações. Sem PII sensível.
    user_agent   text,
    created_at   timestamptz NOT NULL,
    -- Marca subs que já falharam com 410 Gone (endpoint expirou). Deletamos
    -- na próxima limpeza; enquanto isso não tentamos enviar de novo.
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

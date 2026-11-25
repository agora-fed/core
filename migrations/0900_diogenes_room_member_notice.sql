-- Migration 0900 — DIOGENES add-on: tell a person who was added to an articulation room
-- (ADR-0020; range 0900–0949 = add-on host integration, see REGISTRY.md).
--
-- 1. `user_notification.kind` gains `room_member` (bell + web push).
-- 2. The editable e-mail template `diogenes_room_member` (/admin/email-templates).
--    Opt-out: `citizen.email_prefs.room_member = false` (absent = on).
-- Idempotent. Apply as the table owner (`SET ROLE dsoc`).

BEGIN;

ALTER TABLE user_notification DROP CONSTRAINT IF EXISTS user_notification_kind_check;
ALTER TABLE user_notification
    ADD CONSTRAINT user_notification_kind_check
    CHECK (kind IN (
        -- Fediverse (0406).
        'mention', 'reply', 'favourite', 'reblog', 'follow',
        -- Civic (0411).
        'proposal_threshold', 'sla_started', 'sla_response', 'sla_expired',
        -- DIOGENES add-on (0900).
        'room_member'
    ));

INSERT INTO email_template (key, label, subject, body, default_subject, default_body, variables, updated_at)
VALUES (
    'diogenes_room_member',
    'Articulação — você foi adicionado(a) a uma sala',
    '{{added_by}} adicionou você à sala "{{room_name}}"',
    E'Olá,\n\n{{added_by}} adicionou você à sala de articulação "{{room_name}}" como {{role_label}}.\n\nAbra a sala:\n{{room_url}}\n\nO conteúdo da sala é visível apenas para os membros dela.\n\nPara não receber mais este aviso por e-mail, ajuste suas preferências:\n{{prefs_url}}',
    '{{added_by}} adicionou você à sala "{{room_name}}"',
    E'Olá,\n\n{{added_by}} adicionou você à sala de articulação "{{room_name}}" como {{role_label}}.\n\nAbra a sala:\n{{room_url}}\n\nO conteúdo da sala é visível apenas para os membros dela.\n\nPara não receber mais este aviso por e-mail, ajuste suas preferências:\n{{prefs_url}}',
    ARRAY['added_by', 'room_name', 'role_label', 'room_url', 'prefs_url'],
    now()
)
ON CONFLICT (key) DO NOTHING;

COMMIT;

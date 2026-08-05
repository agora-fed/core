-- 0507_moderation_state.sql — per-account moderation state + audit log.
--
-- Actions moderation takes on an account that are visible in the system's
-- behaviour (it is not just ticking a checkbox — the feed, the inbox and the
-- publishing API all read these fields):
--
--  * `suspended_at`  — the account is banned from the instance. It cannot log in, publish,
--                      follow or be found. Active sessions drop on the
--                      next request. `deleted_at` remains reserved
--                      for erasure initiated by the citizen themselves (LGPD).
--  * `silenced_at`   — a soft ban. The citizen's posts do not appear in the public
--                      feed nor in the directory; existing followers keep
--                      receiving them. It does not block login.

ALTER TABLE citizen
    ADD COLUMN IF NOT EXISTS suspended_at timestamptz,
    ADD COLUMN IF NOT EXISTS suspended_reason text,
    ADD COLUMN IF NOT EXISTS silenced_at timestamptz,
    ADD COLUMN IF NOT EXISTS silenced_reason text;

CREATE INDEX IF NOT EXISTS citizen_active_moderation_idx
    ON citizen (id)
    WHERE deleted_at IS NULL AND suspended_at IS NULL;

COMMENT ON COLUMN citizen.suspended_at IS
    '0.26.11: quando NOT NULL, conta suspensa pela moderação. Bloqueia login/publicação/discovery.';
COMMENT ON COLUMN citizen.silenced_at IS
    '0.26.11: quando NOT NULL, posts saem do feed público e do diretório; quem já segue continua vendo.';

-- ─────────────────────────────────────────────────────────────
-- Audit log — moderation actions stay recorded.
-- ─────────────────────────────────────────────────────────────
CREATE TABLE admin_audit (
    id             uuid PRIMARY KEY,
    -- Who took the action. No CASCADE: if the admin is deleted, the record
    -- remains as a trail.
    admin_id       uuid NOT NULL REFERENCES citizen(id),
    -- Categoria fixa. Cobre a lista curta esperada: contas + reports +
    -- federation. It grows with the product.
    action         text NOT NULL CHECK (action IN (
        'account_suspend', 'account_unsuspend',
        'account_silence', 'account_unsilence',
        'account_role_change',
        'report_resolve', 'report_reopen',
        'server_domain_block', 'server_domain_unblock',
        'note_hide'
    )),
    -- Target of the action. It may be a citizen (target_citizen_id), a remote
    -- domain (target_domain), or a report (the generic target_id).
    target_citizen_id  uuid,
    target_domain      text,
    target_id          uuid,
    -- Free JSON with the action's details (new role, before/after, reason,
    -- etc). The historical consumer needs no rigid parsing.
    detail             jsonb,
    created_at         timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX admin_audit_created_idx ON admin_audit (created_at DESC);
CREATE INDEX admin_audit_admin_idx ON admin_audit (admin_id, created_at DESC);
CREATE INDEX admin_audit_target_citizen_idx
    ON admin_audit (target_citizen_id, created_at DESC)
    WHERE target_citizen_id IS NOT NULL;

COMMENT ON TABLE admin_audit IS
    '0.26.11: registro imutável das ações do painel de moderação/admin.';

ALTER TABLE admin_audit OWNER TO dsoc;

-- 0507_moderation_state.sql — estado de moderação por conta + audit log.
--
-- Ações que a moderação faz numa conta e são visíveis no comportamento do
-- sistema (não é só marcar um checkbox — o feed, o inbox e a API de
-- publicação todos leem esses campos):
--
--  * `suspended_at`  — conta banida da instância. Não pode logar, publicar,
--                      seguir ou ser encontrada. Sessões ativas caem no
--                      próximo request. `deleted_at` continua reservado
--                      para a exclusão iniciada pelo próprio cidadão (LGPD).
--  * `silenced_at`   — soft ban. Posts do citizen não aparecem no feed
--                      público nem no diretório; quem já segue continua
--                      recebendo. Não bloqueia login.

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
-- Audit log — ações moderativas ficam registradas.
-- ─────────────────────────────────────────────────────────────
CREATE TABLE admin_audit (
    id             uuid PRIMARY KEY,
    -- Quem tomou a ação. Sem CASCADE: se o admin for excluído, o registro
    -- permanece pra rastro.
    admin_id       uuid NOT NULL REFERENCES citizen(id),
    -- Categoria fixa. Cobre a lista curta esperada: contas + reports +
    -- federação. Cresce com o produto.
    action         text NOT NULL CHECK (action IN (
        'account_suspend', 'account_unsuspend',
        'account_silence', 'account_unsilence',
        'account_role_change',
        'report_resolve', 'report_reopen',
        'server_domain_block', 'server_domain_unblock',
        'note_hide'
    )),
    -- Alvo da ação. Pode ser um citizen (target_citizen_id), um domínio
    -- remoto (target_domain), ou um report (target_id genérico).
    target_citizen_id  uuid,
    target_domain      text,
    target_id          uuid,
    -- JSON livre com detalhes da ação (novo role, antes/depois, reason,
    -- etc). O consumidor histórico não precisa parsing rígido.
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

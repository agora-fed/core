-- Migration 0151 — templates de e-mail editáveis pela UI (0.25.0-fediverso).
--
-- Substitui strings hardcoded em Rust (proposal_delivery, signup_verify,
-- password_reset, mandate_invite) por linhas editáveis. Admin edita título +
-- corpo pela UI; render substitui `{{var}}` server-side com valores do
-- contexto.
--
-- - `key` é o identificador estável (nunca traduzido) usado no código:
--   'proposal_confirm_author', 'signup_verify_cidadao' etc.
-- - `variables` documenta quais placeholders o template aceita — a UI
--   mostra "Variáveis disponíveis" pra o admin não esquecer.
-- - `updated_by` = citizen que editou (pra audit — quem mexeu quando).
-- - `default_subject/body` são a versão hardcoded original; permite "Voltar
--   ao padrão" na UI sem consultar o repo.
--
-- Idempotente: re-run atualiza defaults sem apagar edições do admin.

BEGIN;

CREATE TABLE email_template (
    key              text PRIMARY KEY,
    -- Descrição humana que aparece na lista da UI ("E-mail de confirmação
    -- de cadastro", "E-mail pro gabinete recebendo proposta"…).
    label            text NOT NULL,
    subject          text NOT NULL,
    body             text NOT NULL,
    -- Fallback: se o admin apagar tudo (`subject=''`), render volta pro default.
    default_subject  text NOT NULL,
    default_body     text NOT NULL,
    -- Array de placeholders aceitos, ex.: {'author_name', 'proposal_title', 'proposal_url'}.
    variables        text[] NOT NULL DEFAULT '{}',
    updated_at       timestamptz NOT NULL,
    updated_by       uuid REFERENCES citizen(id),
    created_at       timestamptz NOT NULL DEFAULT now()
);

COMMENT ON TABLE email_template IS
    '0.25.0-fediverso: templates de e-mail editáveis pela UI admin. Chave estável = code path.';

ALTER TABLE email_template OWNER TO dsoc;

-- Seed: as 4 templates que já saem hoje. Text/subject copiados 1:1 do que
-- as crates estão gerando pra o admin não perceber diferença antes de mexer.

INSERT INTO email_template (key, label, subject, body, default_subject, default_body, variables, updated_at)
VALUES
(
    'proposal_confirm_author',
    'Confirmação de proposta pro autor (cidadão que propôs)',
    'Sua proposta foi registrada — {{proposal_title}}',
    E'Olá,\n\nSua proposta "{{proposal_title}}" foi registrada e enviada ao gabinete de {{mandate_name}} para conhecimento.\n\nAcompanhe aqui:\n{{proposal_url}}\n\nQuando outras pessoas apoiarem e o limiar for atingido, o relógio de resposta começa a correr. Silêncio vira registro público.\n\n— DemocraciaBR',
    'Sua proposta foi registrada — {{proposal_title}}',
    E'Olá,\n\nSua proposta "{{proposal_title}}" foi registrada e enviada ao gabinete de {{mandate_name}} para conhecimento.\n\nAcompanhe aqui:\n{{proposal_url}}\n\nQuando outras pessoas apoiarem e o limiar for atingido, o relógio de resposta começa a correr. Silêncio vira registro público.\n\n— DemocraciaBR',
    ARRAY['proposal_title', 'mandate_name', 'proposal_url'],
    now()
),
(
    'proposal_confirm_mandate',
    'Notificação pro gabinete (mandato recebendo proposta)',
    '[DemocraciaBR] Nova proposta cidadã — {{proposal_title}}',
    E'Olá,\n\nVocê recebeu uma nova proposta cidadã pela DemocraciaBR, infraestrutura pública de accountability parlamentar.\n\nTítulo: {{proposal_title}}\n\nTrecho:\n{{proposal_body_short}}\n\nLeia o texto completo, veja o número de apoios e responda:\n{{proposal_url}}\n\nEnviada por cidadã(o) verificada(o) da plataforma. Não é necessário responder por esta caixa — a resposta formal fica registrada dentro do link acima e conta pro placar público de accountability.\n\n— DemocraciaBR (sistema automático)',
    '[DemocraciaBR] Nova proposta cidadã — {{proposal_title}}',
    E'Olá,\n\nVocê recebeu uma nova proposta cidadã pela DemocraciaBR, infraestrutura pública de accountability parlamentar.\n\nTítulo: {{proposal_title}}\n\nTrecho:\n{{proposal_body_short}}\n\nLeia o texto completo, veja o número de apoios e responda:\n{{proposal_url}}\n\nEnviada por cidadã(o) verificada(o) da plataforma. Não é necessário responder por esta caixa — a resposta formal fica registrada dentro do link acima e conta pro placar público de accountability.\n\n— DemocraciaBR (sistema automático)',
    ARRAY['proposal_title', 'proposal_body_short', 'proposal_url'],
    now()
),
(
    'signup_verify',
    'Confirmação de cadastro (verify e-mail)',
    'DemocraciaBR — confirme sua conta',
    E'Olá,\n\nRecebemos seu cadastro na DemocraciaBR. Pra ativar a conta e fazer o primeiro login, abra este link em até 24 horas:\n\n{{confirm_url}}\n\nSe não foi você quem se cadastrou, é só ignorar esta mensagem — a conta nunca é criada sem esta confirmação.\n\n— DemocraciaBR',
    'DemocraciaBR — confirme sua conta',
    E'Olá,\n\nRecebemos seu cadastro na DemocraciaBR. Pra ativar a conta e fazer o primeiro login, abra este link em até 24 horas:\n\n{{confirm_url}}\n\nSe não foi você quem se cadastrou, é só ignorar esta mensagem — a conta nunca é criada sem esta confirmação.\n\n— DemocraciaBR',
    ARRAY['confirm_url'],
    now()
),
(
    'password_reset',
    'Redefinição de senha',
    'DemocraciaBR — redefinição de senha',
    E'Olá,\n\nVocê (ou alguém) pediu para redefinir a senha da sua conta na DemocraciaBR. Para criar uma nova senha, abra este link em até 1 hora:\n\n{{reset_url}}\n\nSe não foi você, ignore esta mensagem — sua senha continua a mesma.\n\n— DemocraciaBR',
    'DemocraciaBR — redefinição de senha',
    E'Olá,\n\nVocê (ou alguém) pediu para redefinir a senha da sua conta na DemocraciaBR. Para criar uma nova senha, abra este link em até 1 hora:\n\n{{reset_url}}\n\nSe não foi você, ignore esta mensagem — sua senha continua a mesma.\n\n— DemocraciaBR',
    ARRAY['reset_url'],
    now()
)
ON CONFLICT (key) DO UPDATE SET
    label            = EXCLUDED.label,
    default_subject  = EXCLUDED.default_subject,
    default_body     = EXCLUDED.default_body,
    variables        = EXCLUDED.variables;
    -- NÃO sobrescrevemos subject/body — respeita edições do admin.

COMMIT;

-- Migration 0535 — templates de e-mail de designação de papel (0.50.0).
--
-- Quando o admin designa admin/moderador de partido ou admin da plataforma, a
-- pessoa recebe um e-mail. Os textos vivem no CATÁLOGO editável (email_template),
-- não hardcoded — assim aparecem e são editáveis no gerenciador do admin
-- (/admin/email-templates) e o corpo texto-plano é embrulhado em HTML da marca
-- pelo `html_wrap`. Placeholders `{{var}}`. Idempotente (ON CONFLICT DO NOTHING).

BEGIN;

INSERT INTO email_template (key, label, subject, body, default_subject, default_body, variables, updated_at)
VALUES
    (
        'role_party_admin',
        'Designação — administrador(a) de partido',
        'Você agora administra o {{party}} na DemocraciaBR',
        E'Olá,\n\nVocê foi designado(a) como administrador(a) do diretório do {{party}} na DemocraciaBR.\n\nComo administrador(a) de partido, você pode:\n\n\t• organizar os diretórios do partido (nacional, estaduais e municipais);\n\t• atestar mandatos e candidaturas do partido;\n\t• coordenar a presença do {{party}} na plataforma.\n\nVeja o espaço do partido:\n{{party_url}}\n\nSe você não esperava por esta designação, responda este e-mail — nós verificamos.\n\n— DemocraciaBR',
        'Você agora administra o {{party}} na DemocraciaBR',
        E'Olá,\n\nVocê foi designado(a) como administrador(a) do diretório do {{party}} na DemocraciaBR.\n\nComo administrador(a) de partido, você pode:\n\n\t• organizar os diretórios do partido (nacional, estaduais e municipais);\n\t• atestar mandatos e candidaturas do partido;\n\t• coordenar a presença do {{party}} na plataforma.\n\nVeja o espaço do partido:\n{{party_url}}\n\nSe você não esperava por esta designação, responda este e-mail — nós verificamos.\n\n— DemocraciaBR',
        ARRAY['party', 'party_url'],
        now()
    ),
    (
        'role_party_moderador',
        'Designação — moderador(a) de partido',
        'Você agora modera o {{party}} na DemocraciaBR',
        E'Olá,\n\nVocê foi designado(a) como moderador(a) do {{party}} na DemocraciaBR.\n\nComo moderador(a), você ajuda a manter a qualidade do espaço do partido e pode atestar mandatos e candidaturas. (Reorganizar os diretórios continua sendo função do administrador do partido.)\n\nVeja o espaço do partido:\n{{party_url}}\n\nSe você não esperava por esta designação, responda este e-mail — nós verificamos.\n\n— DemocraciaBR',
        'Você agora modera o {{party}} na DemocraciaBR',
        E'Olá,\n\nVocê foi designado(a) como moderador(a) do {{party}} na DemocraciaBR.\n\nComo moderador(a), você ajuda a manter a qualidade do espaço do partido e pode atestar mandatos e candidaturas. (Reorganizar os diretórios continua sendo função do administrador do partido.)\n\nVeja o espaço do partido:\n{{party_url}}\n\nSe você não esperava por esta designação, responda este e-mail — nós verificamos.\n\n— DemocraciaBR',
        ARRAY['party', 'party_url'],
        now()
    ),
    (
        'role_platform',
        'Designação — papel de plataforma (admin/owner/auditor)',
        'Você agora é {{role_label}} da DemocraciaBR',
        E'Olá,\n\nVocê recebeu o papel de {{role_label}} da plataforma DemocraciaBR.\n\nEste é um papel de confiança: dá acesso ao painel administrativo — contas, moderação, denúncias, auditoria e configurações da plataforma. Use com responsabilidade; toda ação administrativa fica registrada na trilha de auditoria.\n\nAcesse o painel:\n{{admin_url}}\n\nSe você não esperava por esta designação, responda este e-mail imediatamente.\n\n— DemocraciaBR',
        'Você agora é {{role_label}} da DemocraciaBR',
        E'Olá,\n\nVocê recebeu o papel de {{role_label}} da plataforma DemocraciaBR.\n\nEste é um papel de confiança: dá acesso ao painel administrativo — contas, moderação, denúncias, auditoria e configurações da plataforma. Use com responsabilidade; toda ação administrativa fica registrada na trilha de auditoria.\n\nAcesse o painel:\n{{admin_url}}\n\nSe você não esperava por esta designação, responda este e-mail imediatamente.\n\n— DemocraciaBR',
        ARRAY['role_label', 'admin_url'],
        now()
    )
ON CONFLICT (key) DO NOTHING;

COMMIT;

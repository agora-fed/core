-- Migration 0535 — role-assignment e-mail templates (0.50.0).
--
-- When the admin assigns a party admin/moderator or a platform admin, the
-- person receives an e-mail. The texts live in the editable CATALOGUE (email_template),
-- not hardcoded — so they show up and can be edited in the admin manager
-- (/admin/email-templates) and the plain-text body is wrapped in branded HTML
-- by `html_wrap`. Placeholders `{{var}}`. Idempotent (ON CONFLICT DO NOTHING).

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

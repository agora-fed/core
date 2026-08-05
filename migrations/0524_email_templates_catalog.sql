-- Migration 0524 — the complete e-mail template catalog (0.32.0).
--
-- A 0151 criou a tabela e seedou 4 templates; este migration completa o
-- catalog: every e-mail the platform sends (or starts sending in this
-- version) has a row the admin can edit at /admin/email-templates.
--
-- New e-mails introduced in 0.32.0:
-- - welcome                    → welcome after the account is activated
-- - follow_new                 → someone followed you (gated by email_prefs.follow)
-- - sla_started_mandate        → D0 to the office when the SLA starts (with the
--                                answer-without-an-account link). It also records
--                                receipt #1 — without it the worker's D+1/D+2
--                                "digital registered mail" never fired.
-- - proposal_threshold_author  → sua proposta cruzou o gatilho
-- - sla_response_author        → o mandato respondeu
-- - sla_expired_author         → public silence recorded
--
-- Already hardcoded in the code and now becoming templates:
-- - mandate_invite             → invitation to claim a mandate (dsoc-auth)
-- - sla_reminder_mandate       → D+1/D+2 reminders to the cabinet (worker)
--
-- Idempotent: a re-run updates the defaults without wiping the admin's edits
-- (the same pattern as 0151).

BEGIN;

INSERT INTO email_template (key, label, subject, body, default_subject, default_body, variables, updated_at)
VALUES
(
    'welcome',
    'Boas-vindas (conta ativada)',
    'Bem-vindo(a) à DemocraciaBR — sua conta está ativa',
    E'Olá,\n\nSua conta na DemocraciaBR está ativa. A plataforma é infraestrutura pública de accountability: aqui você propõe, apoia, acompanha o placar dos mandatos — e o silêncio dos gabinetes vira registro público.\n\nPrimeiros passos:\n\n\t• Complete seu perfil e preferências: {{settings_url}}\n\t• Encontre seus representantes: {{site_url}}/politicos/\n\t• Veja o que a comunidade está propondo: {{site_url}}/propostas/\n\nSua conta também é um endereço no fediverso — pessoas no Mastodon e em toda a rede federada podem te seguir.\n\n— DemocraciaBR',
    'Bem-vindo(a) à DemocraciaBR — sua conta está ativa',
    E'Olá,\n\nSua conta na DemocraciaBR está ativa. A plataforma é infraestrutura pública de accountability: aqui você propõe, apoia, acompanha o placar dos mandatos — e o silêncio dos gabinetes vira registro público.\n\nPrimeiros passos:\n\n\t• Complete seu perfil e preferências: {{settings_url}}\n\t• Encontre seus representantes: {{site_url}}/politicos/\n\t• Veja o que a comunidade está propondo: {{site_url}}/propostas/\n\nSua conta também é um endereço no fediverso — pessoas no Mastodon e em toda a rede federada podem te seguir.\n\n— DemocraciaBR',
    ARRAY['site_url', 'settings_url'],
    now()
),
(
    'follow_new',
    'Novo seguidor (alguém te seguiu)',
    '{{follower_name}} começou a seguir você na DemocraciaBR',
    E'Olá,\n\n{{follower_name}} ({{follower_handle}}) começou a seguir você na DemocraciaBR.\n\nVer o perfil de quem te seguiu:\n{{follower_url}}\n\nSuas notificações:\n{{notifications_url}}\n\nPra deixar de receber este aviso por e-mail, desligue "novo seguidor" em Configurações → Preferências.\n\n— DemocraciaBR',
    '{{follower_name}} começou a seguir você na DemocraciaBR',
    E'Olá,\n\n{{follower_name}} ({{follower_handle}}) começou a seguir você na DemocraciaBR.\n\nVer o perfil de quem te seguiu:\n{{follower_url}}\n\nSuas notificações:\n{{notifications_url}}\n\nPra deixar de receber este aviso por e-mail, desligue "novo seguidor" em Configurações → Preferências.\n\n— DemocraciaBR',
    ARRAY['follower_name', 'follower_handle', 'follower_url', 'notifications_url'],
    now()
),
(
    'mandate_invite',
    'Convite pra assumir mandato (e-mail ao gabinete)',
    'DemocraciaBR — convite para assumir o mandato de {{mandate_name}}',
    E'Olá,\n\nVocê foi convidado(a) a assumir, na plataforma DemocraciaBR, o mandato de:\n\n\t{{mandate_name}} ({{party_uf}} — {{office}})\n\nA DemocraciaBR é uma plataforma cidadã de cobrança pública de mandatos. Ao aceitar este convite você:\n\n\t• Cria sua conta pessoal (e-mail, senha e CPF).\n\t• Verifica sua identidade no mandato indicado acima (nível ''directory'').\n\t• Torna seu perfil público — a transparência é a moeda desta plataforma.\n\nEste link é único, expira em {{hours}} horas e só pode ser usado uma vez:\n\n{{accept_url}}\n\nSe você não reconhece este convite, ignore este e-mail — nada é feito até você abrir o link acima.\n\n— DemocraciaBR',
    'DemocraciaBR — convite para assumir o mandato de {{mandate_name}}',
    E'Olá,\n\nVocê foi convidado(a) a assumir, na plataforma DemocraciaBR, o mandato de:\n\n\t{{mandate_name}} ({{party_uf}} — {{office}})\n\nA DemocraciaBR é uma plataforma cidadã de cobrança pública de mandatos. Ao aceitar este convite você:\n\n\t• Cria sua conta pessoal (e-mail, senha e CPF).\n\t• Verifica sua identidade no mandato indicado acima (nível ''directory'').\n\t• Torna seu perfil público — a transparência é a moeda desta plataforma.\n\nEste link é único, expira em {{hours}} horas e só pode ser usado uma vez:\n\n{{accept_url}}\n\nSe você não reconhece este convite, ignore este e-mail — nada é feito até você abrir o link acima.\n\n— DemocraciaBR',
    ARRAY['mandate_name', 'party_uf', 'office', 'hours', 'accept_url'],
    now()
),
(
    'sla_started_mandate',
    'SLA começou — 1º aviso ao gabinete (D0, com link de resposta)',
    '[DemocraciaBR] Prazo de resposta iniciado — {{proposal_title}}',
    E'Prezado(a) {{mandate_name}},\n\nA proposta cidadã "{{proposal_title}}" atingiu o número de apoios necessário e o prazo público de resposta do seu gabinete começou a contar. O prazo encerra em {{due_date}}.\n\nResponder agora, sem cadastro (link exclusivo desta caixa):\n{{respond_url}}\n\nVer a demanda completa e os apoios:\n{{proposal_url}}\n\nCada aviso enviado fica registrado publicamente com recibo verificável. A resposta do gabinete — ou o silêncio ao fim do prazo — entra no placar público de accountability do mandato.\n\n— DemocraciaBR (sistema automático)',
    '[DemocraciaBR] Prazo de resposta iniciado — {{proposal_title}}',
    E'Prezado(a) {{mandate_name}},\n\nA proposta cidadã "{{proposal_title}}" atingiu o número de apoios necessário e o prazo público de resposta do seu gabinete começou a contar. O prazo encerra em {{due_date}}.\n\nResponder agora, sem cadastro (link exclusivo desta caixa):\n{{respond_url}}\n\nVer a demanda completa e os apoios:\n{{proposal_url}}\n\nCada aviso enviado fica registrado publicamente com recibo verificável. A resposta do gabinete — ou o silêncio ao fim do prazo — entra no placar público de accountability do mandato.\n\n— DemocraciaBR (sistema automático)',
    ARRAY['mandate_name', 'proposal_title', 'due_date', 'respond_url', 'proposal_url'],
    now()
),
(
    'sla_reminder_mandate',
    'Lembrete D+1/D+2 ao gabinete (SLA pendente)',
    '[Lembrete {{attempt}}/3] Demanda cidadã aguardando resposta — {{proposal_title}}',
    E'Prezado(a) {{mandate_name}},\n\nA demanda cidadã "{{proposal_title}}" segue aguardando resposta do gabinete. Este é o {{attempt}}º aviso; cada aviso fica registrado publicamente com recibo verificável.\n\nResponder agora (sem cadastro): {{respond_url}}\n\nVer a demanda: {{proposal_url}}\n\n— DemocraciaBR',
    '[Lembrete {{attempt}}/3] Demanda cidadã aguardando resposta — {{proposal_title}}',
    E'Prezado(a) {{mandate_name}},\n\nA demanda cidadã "{{proposal_title}}" segue aguardando resposta do gabinete. Este é o {{attempt}}º aviso; cada aviso fica registrado publicamente com recibo verificável.\n\nResponder agora (sem cadastro): {{respond_url}}\n\nVer a demanda: {{proposal_url}}\n\n— DemocraciaBR',
    ARRAY['mandate_name', 'proposal_title', 'attempt', 'respond_url', 'proposal_url'],
    now()
),
(
    'proposal_threshold_author',
    'Gatilho cruzado (aviso ao autor da proposta)',
    '🚨 Sua proposta cruzou o gatilho — {{proposal_title}}',
    E'Olá,\n\nSua proposta "{{proposal_title}}" atingiu o número de apoios necessário. O gabinete responsável foi notificado e o prazo público de resposta começou a contar.\n\nAcompanhe aqui — cada aviso ao gabinete fica registrado com recibo verificável:\n{{proposal_url}}\n\nSe o gabinete responder, você é avisado(a). Se ficar em silêncio até o fim do prazo, o silêncio vira registro público permanente no placar do mandato.\n\n— DemocraciaBR',
    '🚨 Sua proposta cruzou o gatilho — {{proposal_title}}',
    E'Olá,\n\nSua proposta "{{proposal_title}}" atingiu o número de apoios necessário. O gabinete responsável foi notificado e o prazo público de resposta começou a contar.\n\nAcompanhe aqui — cada aviso ao gabinete fica registrado com recibo verificável:\n{{proposal_url}}\n\nSe o gabinete responder, você é avisado(a). Se ficar em silêncio até o fim do prazo, o silêncio vira registro público permanente no placar do mandato.\n\n— DemocraciaBR',
    ARRAY['proposal_title', 'proposal_url'],
    now()
),
(
    'sla_response_author',
    'Mandato respondeu (aviso ao autor da proposta)',
    '✅ O mandato respondeu sua proposta — {{proposal_title}}',
    E'Olá,\n\nO gabinete de {{mandate_name}} respondeu à sua proposta "{{proposal_title}}". A resposta está registrada publicamente e já conta no placar de accountability do mandato.\n\nLer a resposta:\n{{proposal_url}}\n\n— DemocraciaBR',
    '✅ O mandato respondeu sua proposta — {{proposal_title}}',
    E'Olá,\n\nO gabinete de {{mandate_name}} respondeu à sua proposta "{{proposal_title}}". A resposta está registrada publicamente e já conta no placar de accountability do mandato.\n\nLer a resposta:\n{{proposal_url}}\n\n— DemocraciaBR',
    ARRAY['proposal_title', 'proposal_url', 'mandate_name'],
    now()
),
(
    'sla_expired_author',
    'Silêncio registrado (aviso ao autor da proposta)',
    '🔇 Silêncio público registrado — {{proposal_title}}',
    E'Olá,\n\nO prazo de resposta da sua proposta "{{proposal_title}}" venceu sem manifestação do gabinete de {{mandate_name}}. O silêncio agora é registro público permanente no placar do mandato — com a linha do tempo de todos os avisos enviados e seus recibos verificáveis.\n\nVer o registro:\n{{proposal_url}}\n\n— DemocraciaBR',
    '🔇 Silêncio público registrado — {{proposal_title}}',
    E'Olá,\n\nO prazo de resposta da sua proposta "{{proposal_title}}" venceu sem manifestação do gabinete de {{mandate_name}}. O silêncio agora é registro público permanente no placar do mandato — com a linha do tempo de todos os avisos enviados e seus recibos verificáveis.\n\nVer o registro:\n{{proposal_url}}\n\n— DemocraciaBR',
    ARRAY['proposal_title', 'proposal_url', 'mandate_name'],
    now()
)
ON CONFLICT (key) DO UPDATE SET
    label            = EXCLUDED.label,
    default_subject  = EXCLUDED.default_subject,
    default_body     = EXCLUDED.default_body,
    variables        = EXCLUDED.variables;
    -- We do NOT overwrite subject/body — it respects the admin's edits.

COMMIT;

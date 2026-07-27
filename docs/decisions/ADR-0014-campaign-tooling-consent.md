# ADR-0014 — Camada de campanha (ÁGORA): dois pools de contato consentidos, SMS por diretório

- **Status:** Accepted
- **Context:** Visão do Marcos (2026-07-27); épico em git.pop.coop/brasil/democracia-social#57
  (sub-issues #58–#64). Nomenclatura em inglês por [[ADR-0013]].

## Decision

Transformar o ÁGORA em infraestrutura de campanha que **diretórios partidários e candidatos
abastecem com dados** (verificados contra a base central) em troca de ferramentas de
Participação Popular. Fundação já no schema: `party`, `party_directory`, `party_administrator`
(0204), `audience_contact` (0525), `campaign_groups` (0527), eixo território (município IBGE).

### 1. Dois pools de contato com bases legais distintas
- **Base própria** (candidato/diretório *sobe* a lista): **controlador = eles**; tabela
  **isolada por candidato×campanha** (LGPD-apagável em bloco); verificada/deduplicada contra a
  base central; opt-out do cidadão sempre suprime.
- **Base global** (cadastros do ÁGORA): alcançável **só como broadcast mediado e consentido** —
  **a lista crua nunca é exportada**; a plataforma envia em nome do diretório.

### 2. Consentimento específico (LGPD art. 11 — dados sensíveis), default OFF, 4 níveis
O cidadão autoriza compartilhar seus dados para comunicação de campanha em UM nível:
1. todos os partidos; 2. um partido específico; 3. todos os partidos de um município; 4. um
diretório de um município. Revogável; opt-out de 1 clique; auditável.

### 3. Canais e limites
- **E-mail** primeiro (reusa `mailer::send_html` + padrão de lote do `invite_campaign`).
- **SMS via SMSGateway** (app Android sms-gate.app), **por diretório/campanha** — cada um
  cadastra seu próprio `host`/`api_token` nas configurações (cifrado em repouso). `SmsSender`
  plugável (padrão do `CpfVerifier`).
- **Limite: 1 SMS/semana por destinatário** para diretórios e candidatos; **OWNER da
  plataforma sem limite**.
- **Telefone** é opt-in, verificado por OTP SMS **após** o e-mail, só se o usuário optar por
  notificações SMS; comunicado como 2FA **não recomendada** para perda de e-mail. **TOTP**
  (RFC 6238) é o 2FA recomendado, opcional.

### 4. Diferencial: amarração com o loop de consequência
A micro-consulta municipal do candidato pode ser **promovida a Consulta pública** (reusa
`consultations`), cujas respostas viram propostas com SLA ao gabinete. É o fosso competitivo —
nenhum Mailchimp entrega sinal cívico.

## Rationale

Reusar contatos de cidadãos (coletados para participação cívica) em campanha de terceiros sem
consentimento específico violaria a LGPD (art. 11, dados sensíveis) e envenenaria a adesão
(spam → churn). O consentimento granular **é** o produto: vários partidos alcançam a base, mas
só quem optou por ouvir, com a plataforma como intermediária auditável. A isolação por
candidato×campanha resolve a titularidade/controladoria e o apagamento LGPD.

## Consequences

- F2/F3 dependem de revisão jurídica antes de ir ao ar (consentimento destacado).
- SMSGateway não escala (um aparelho/SIM) → escopo baixo volume (2FA + município pequeno).
- "Verificar contra a base central" é forte por CPF, parcial por e-mail/telefone.
- Ordem de entrega recomendada: F1 (papéis) → F2 (consent) → F3 (broadcast/micro-consulta) →
  F4 (upload) → F5 (SMS) → F6 (2FA) → F7 (painéis).

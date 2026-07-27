# ADR-0016 — INTERCOMS: vertical de comunicação externa com providers plugáveis

- **Status:** Accepted
- **Context:** Diretriz do Marcos (2026-07-27). Refina o canal SMS da camada de campanha [[ADR-0014]].
  Nomenclatura inglês por [[ADR-0013]].

## Decision

**INTERCOMS** é a vertical de **comunicação externa** do ÁGORA — uma camada única e plugável por
onde passa todo envio outbound (e-mail e SMS hoje; o que vier depois). Providers implementam uma
interface comum; o subsistema de campanha e o de auth (OTP/2FA) consomem INTERCOMS, não um
provider específico.

- **Interface (inglês):** `trait MessageSender { async fn send(&self, msg) -> Result }`, com
  `Channel { Email, Sms }`.
- **Providers:**
  - E-mail: **SMTP** (o `mailer::send_html`/lettre atual vira o `SmtpProvider`), **MailGun**,
    e serviços de **disparo em massa** (futuros).
  - SMS: **SMSGateway** (app Android sms-gate.app — 1º provider), outros SMS depois.
- **Config por escopo:** plataforma / **diretório partidário** / **campanha** registram seu
  próprio provider + credenciais (ex.: o diretório com seu próprio SMSGateway `host`/`api_token`).
  Credenciais **cifradas em repouso**, nunca em log. Um registry resolve o provider por
  (escopo, canal).
- **Rate limits vivem no INTERCOMS:** SMS **1/semana por destinatário** para diretório/candidato;
  **OWNER da plataforma sem limite** (regra do ADR-0014 movida pra cá).

## Rationale

Sem esta camada, cada feature (2FA, broadcast de campanha, SLA-ao-gabinete) reimplementaria envio
e prenderia-se a um provider. Uma vertical única com providers plugáveis deixa trocar SMSGateway↔
MailGun↔disparo-em-massa sem tocar as features, e permite que cada diretório traga sua própria
infra de envio (soberania + custo por conta de quem usa). Espelha o `CpfVerifier`/`IdentityVerifier`
(ADR-0015): a feature fala com o trait, a instalação/escopo escolhe a impl.

## Consequences

- Refactor: `mailer::send_html` → `SmtpProvider` sob INTERCOMS; migrar os envios atuais
  (password-reset, signup-verify, contato, SLA, mandate-invite) para o `MessageSender`.
- F5 da camada de campanha (SMS) nasce como `SmsGatewayProvider` no INTERCOMS, não avulso.
- Config cifrada por escopo exige uma tabela `intercoms_provider_config` (escopo + canal +
  provider + segredo cifrado) e uma chave de cifra no Secret.
- OTP/2FA (F6) e broadcast de campanha (F3) passam a mandar via INTERCOMS.

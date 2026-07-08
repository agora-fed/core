# ADR-0012 — Abandonar Zitadel de vez (revoga o remanescente de OIDC)

- **Status:** Accepted · **Supersedes:** ADR-0008 (parcial — trecho "Zitadel/OIDC remains available
  dormant, env-gated"). Fecha o cerco iniciado por ADR-0008 revogando também o path staff/admin.
  Justificado por PLAN.md princípio 12.

## Contexto
ADR-0008 (0.9.0) moveu o path do **cidadão** pra credencial sovereign (e-mail + senha + CPF), mas
deixou `POST /auth/session` + validador OIDC "dormente" pra um eventual SSO de staff via Zitadel.
Na prática, entre 0.9.0 e 0.25.0:

- Nenhum ambiente subiu com `AUTH_OIDC_ISSUER`/`AUTH_OIDC_JWKS_URL` configurados. Prod loga em cada
  startup: `AUTH_OIDC_ISSUER / AUTH_OIDC_JWKS_URL are not configured; /auth endpoints will reject`
  (nível ERROR — polui log de saúde real).
- Admin bootstrap (`docs/ops/ADMIN.md`) usa o path de credencial + upgrade manual do
  `verification_level`, não passa por Zitadel.
- A federação (ADR-0010), o cadastro com verify (0.25.0-fediverso-verify), e o rate-limit de login
  (0.25.0-fediverso-defense) assumem cred sovereign como única surface auth ativa.

Manter o esqueleto OIDC é overhead sem retorno: dependência (`jsonwebtoken`), path morto no HTTP
(`/auth/session`), key source JWKS que não tem quem falar com ela, e um erro chamativo em toda
inicialização.

## Decisão
- **PLAN.md princípio 7** ("Sovereign auth via Zitadel OIDC") formalmente **revogado** — não é mais
  uma diretriz frozen. O sistema é cred sovereign, ponto.
- Rotas `/auth/session` e `/auth/me` continuam existindo por compatibilidade da surface Mastodon
  (usam cookie ou bearer, nunca OIDC — o path OIDC já era no-op na prática).
- `JwksKeySource` / `TokenValidator`/`AUTH_OIDC_*` env vars: **marcadas dormentes** — código não é
  removido *nesta* fatia (evita conflito de merge com trabalho em curso), mas o log ERROR de
  "unconfigured" vira INFO (não polui saúde). Remoção completa fica pra uma futura poda.
- Para **staff/admin SSO no futuro**, a decisão é: se surgir a demanda, faremos uma ADR nova
  avaliando **Keycloak** ou **outra opção sovereign IPv6-friendly**, sem retomar Zitadel.

## Se isto reverte decisão anterior (PLAN.md princípio 12)
- **(a) Por que o approach anterior falha:** Zitadel não trouxe nenhum benefício realizado em
  0.9.0–0.25.0. Falhou no critério prático "quem opera precisa disso?" — ninguém liga.
- **(b) Salvável?** Só se surgir demanda de SSO staff — e nesse caso Keycloak resolve com menos
  fricção operacional (releases ativas, docs mais amplas na comunidade brasileira, imagem IPv6-first).
- **(c) Por que o novo caminho é melhor:** menos superfície de dependência, menos código morto,
  menos ruído no log de prod. A opção de SSO staff continua aberta via ADR futura.

## Consequências
- CHANGELOG: entrada "Abandoned Zitadel/OIDC path (ADR-0012)".
- `crates/platform/auth/src/http.rs`: `validator_from_env` — quando não configurado, loga `INFO`
  ao invés de `ERROR` (o warning já sinaliza "endpoints OIDC vão rejeitar", que é o comportamento
  esperado agora).
- Follow-up (fatia futura, opcional): remover `dsoc_auth::domain::TokenValidator`, `KeySource`,
  `JwksKeySource`, o corpo do handler `create_session`, e os testes que exercitam o path OIDC.

# Coordenação: Agente-App ⇆ Agente-Backend/Site

Dois agentes trabalham neste monorepo canônico (git.pop.coop/brasil/democracia-social):
- **Agente-Backend/Site** — dono de `crates/**` (Rust), `web/**` (Astro), `migrations/**`, e do **deploy do gateway** em produção.
- **Agente-App** — dono do repo separado **`brasil/democracia-app`** (Flutter). Só toca no backend para **endpoints de suporte mobile** (abaixo), sempre commitando aqui.

## Regras de ouro (o incidente de 2026-07-25 violou #1 e #2)
1. **Nada fora do git.** Todo trabalho é commitado + pushado imediatamente. Trabalho não-commitado no working tree será **perdido** quando o outro agente buildar/deployar. (Foi o que derrubou o `/me/whoami`.)
2. **`git pull --rebase origin main` ANTES de todo build/deploy**, e **deploy sempre do HEAD do `origin/main`** — nunca de estado local divergente. Assim toda imagem tem o trabalho dos DOIS.
3. **Não quebrar o contrato mobile** (endpoints abaixo) sem avisar aqui. Há testes que falham no CI do backend se algum sumir.
4. **Uma mudança cross-cutting? Anote neste arquivo** (seção Handoff) no mesmo commit.

## Contrato de API que o APP depende (NÃO remover/quebrar o shape)
- `POST /api/v1/apps` (form) → `{client_id, client_secret}` — registro OAuth do app.
- `POST /oauth/token` (form, grant `password`) → `{access_token}` — login do app (bearer 30d).
- `GET  /api/v1/me/whoami` (Bearer) → `{citizen_id, handle, display_name, verification_level, titulo_status, is_admin, platform_role, party_role, civic_type, mandate}` — **decide a navegação por papel do app**. Guardado por `crates/gateway/tests/http_surface.rs` (testes `whoami_*`).
- `GET  /api/v1/me/feed`, `/me/notifications`, `/proposals`, `/consultas`, `/debates`, `/campaign-groups/*`, `/politicos/*`, `/scorecards/*` — telas do app (leitura).

## Handoff (log de mudanças cross-cutting — mais recente no topo)
- 2026-07-25 (App): adicionado `GET /api/v1/me/whoami` (`crates/gateway/src/whoami.rs`) + testes. App em produção depende dele. **Não remover.**

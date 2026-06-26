# Admin — criação e bootstrap

> Como tornar alguém administrador da plataforma, o modelo atual de autorização, e os gaps a fechar.

## Modelo atual (como o código decide acesso admin)

- Os endpoints `/api/v1/admin/*` (criar org, vincular role, feature flags) chamam
  `authorize_mutation` → `authz.require(org, actor, Directory)`. Ou seja, **acesso de mutação admin é
  por NÍVEL DE VERIFICAÇÃO ≥ `Directory`** (`crates/platform/admin/src/domain.rs`: `MIN_MUTATION_LEVEL`).
- O `actor` (cidadão chamador) vem do header **`x-citizen-id`** (UUID do cidadão).
- A tabela `admin_role_binding` (roles `owner`/`admin`/`auditor`) é **persistida mas ainda NÃO é
  verificada** para acesso — hoje o gate é o nível, não a role. (Ver "Gaps" abaixo.)

Como um cidadão recém-registrado fica em nível **`email`** (abaixo de `Directory`), ele **não** passa
no gate — é exatamente por isso que "falha o acesso como admin".

## Processo de criação de admin (hoje)

1. **A pessoa se registra** normalmente em https://democracia.social.br/cadastrar (e-mail + senha + CPF).
   Isso cria o `citizen` (nível `email`) e a `auth_credential`.
2. **Bootstrap (operação privilegiada, manual)** — eleva esse cidadão a admin. Como não existe (ainda)
   um fluxo de API para criar o *primeiro* admin, faz-se direto no banco, na VM:

   ```sh
   ./scripts/bootstrap-admin.sh <email> [org_id]
   ```
   O script: encontra o cidadão pelo e-mail, sobe `verification_level` para `strong` (≥ Directory),
   e insere um `admin_role_binding` com role `owner` (registro de intenção). É uma decisão soberana do
   operador da instância — concede poder administrativo deliberadamente.

3. **Usar os endpoints admin** enviando o header `x-citizen-id: <uuid-do-cidadão>`:
   ```sh
   curl -X POST https://democracia.social.br/api/v1/admin/orgs/<org>/roles \
     -H 'content-type: application/json' -H 'x-citizen-id: <uuid-admin>' \
     -d '{"citizen_id":"<uuid-alvo>","role":"admin"}'
   ```
   (Depois de fechar os gaps, isso virá automático da sessão — sem header manual.)

## Gaps a fechar (próximos passos recomendados)

1. **Middleware de autenticação no gateway**: validar o token/sessão (de `/auth/login`) e injetar o
   header de identidade padronizado para os handlers — hoje o gateway não faz isso, então a identidade
   precisa ser passada manualmente. Deve ler a sessão e setar `x-dsoc-citizen-id`/`x-dsoc-org-id`
   (extractor `CallerId`, ADR-0007) e, por compat, `x-citizen-id` para o admin.
2. **Padronizar o header**: admin usa `x-citizen-id`; o resto usa `x-dsoc-citizen-id`. Unificar.
3. **Enforçar `admin_role_binding`**: trocar o gate "por nível" por "tem role admin/owner no org",
   para que a role realmente conceda acesso (e o nível seja só o piso de identidade).
4. **UI de admin**: não há tela de admin no front ainda (só `/entrar` e `/cadastrar`).

Até esses itens, o bootstrap acima é o caminho suportado.

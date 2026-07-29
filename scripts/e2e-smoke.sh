#!/usr/bin/env bash
# e2e-smoke.sh — smoke-test de ponta-a-ponta ao vivo (API pública), sem navegador.
# Garante que o funcionamento essencial da plataforma não regrediu após um deploy.
# Uso: BASE=https://democracia.social.br scripts/e2e-smoke.sh   (default = prod)
set -uo pipefail
BASE="${BASE:-https://democracia.social.br}"
API="$BASE/api/v1"
ORG="11111111-1111-1111-1111-111111111111"
pass=0; fail=0
ck() { # ck "nome" "esperado" "obtido"
  if [ "$2" = "$3" ]; then echo "  ✓ $1"; pass=$((pass+1));
  else echo "  ✗ $1 — esperado [$2] obtido [$3]"; fail=$((fail+1)); fi
}
code() { curl -s -o /dev/null -w "%{http_code}" "$@"; }

echo "== E2E smoke @ $BASE =="

echo "[páginas públicas]"
ck "landing 200"        200 "$(code "$BASE/")"
ck "/politicos 200"     200 "$(code "$BASE/politicos/")"
ck "/f (fóruns) 200"    200 "$(code "$BASE/f/")"
ck "/propostas 200"     200 "$(code "$BASE/propostas/")"

echo "[integridade — mandato alcançável vs placeholder (A1)]"
FED=$(curl -s "$API/mandates?org_id=$ORG&limit=1&sphere=federal" | python3 -c "import sys,json;d=json.load(sys.stdin);a=(d.get('data') or d);print(a[0]['is_reachable'] if a else 'none')" 2>/dev/null)
ck "federal is_reachable=true" "True" "$FED"

echo "[cadastro — obrigatoriedade + verificação mantida]"
ck "register cidadão incompleto → 400" 400 "$(code -X POST "$API/auth/register" -H 'content-type: application/json' -d "{\"org_id\":\"$ORG\",\"email\":\"smoke-e2e@example.com\",\"password\":\"senha12345\",\"cpf\":\"11144477735\"}")"

echo "[admin — tudo gated (401 sem sessão)]"
for p in users-rich civic-sources interest-areas consultations politico-contacts/overview; do
  ck "admin/$p → 401" 401 "$(code "$API/admin/$p")"
done

echo "[perfil/gate obrigatório]"
ck "me/profile-status → 401" 401 "$(code "$API/me/profile-status")"
ck "me/interests → 401"      401 "$(code "$API/me/interests")"

echo "[fórum — placar por pontos, sem ponderação (ADR-0019)]"
HASSCORE=$(curl -s "$API/f/topics/d1111111-1111-1111-1111-111111111111" | python3 -c "import sys,json;d=json.load(sys.stdin);t=(d.get('data') or {});print('ok' if isinstance(t.get('score'), int) else 'bad')" 2>/dev/null)
ck "tópico tem placar por pontos (score)" "ok" "$HASSCORE"

echo "[territórios (cadastro)]"
NMUN=$(curl -s "$API/municipios?uf=SP" | python3 -c "import sys,json;d=json.load(sys.stdin);a=(d.get('data') or d);print(len(a))" 2>/dev/null)
ck "municípios SP = 645" 645 "$NMUN"

echo ""
echo "== RESULTADO: $pass ok / $fail falhas =="
[ "$fail" -eq 0 ]

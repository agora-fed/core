#!/usr/bin/env bash
# 3a passada: logos que são arquivos fair-use LOCAIS na pt.wikipedia (não
# Commons) — pega via prop=images (acha o File com "logo" no nome) + imageinfo.
set -uo pipefail

BUCKET="dsoc/dsoc-media"
MEDIA_BASE="https://democracia.social.br/media"
ORG="11111111-1111-1111-1111-111111111111"
OUT_SQL="/tmp/party-logos-3.sql"
: > "$OUT_SQL"
API="https://pt.wikipedia.org/w/api.php"

declare -a PARTIES=(
  "REPUBLICANOS|Republicanos (partido político)"
  "PSB|Partido Socialista Brasileiro"
  "AVANTE|Avante (partido político)"
  "CIDADANIA|Cidadania (partido político)"
  "REDE|Rede Sustentabilidade"
)

enc(){ python3 -c "import urllib.parse,sys;print(urllib.parse.quote(sys.argv[1]))" "$1"; }

for entry in "${PARTIES[@]}"; do
  sigla="${entry%%|*}"; title="${entry#*|}"
  # 1) lista os File: da página, escolhe o que parece logo.
  file=$(curl -s --max-time 15 "$API?action=query&format=json&prop=images&imlimit=500&redirects=1&titles=$(enc "$title")" | SIGLA="$sigla" python3 -c "
import sys,json,re,os
sig=os.environ.get('SIGLA','')
try:
    d=json.load(sys.stdin)
    imgs=[i['title'] for p in d['query']['pages'].values() for i in p.get('images',[])]
    cand=[t for t in imgs if re.search(r'logo|logotipo|s[ii]mbolo', t, re.I) and re.search(r'\.(svg|png|jpg|jpeg)$', t, re.I)]
    cand.sort(key=lambda t: (0 if re.search(re.escape(sig), t, re.I) else 1, len(t)))
    print(cand[0] if cand else '')
except Exception:
    pass
")
  if [ -z "$file" ]; then echo "SKIP $sigla — sem File de logo"; continue; fi
  # 2) URL real do arquivo (aceita local fair-use).
  url=$(curl -s --max-time 15 "$API?action=query&format=json&prop=imageinfo&iiprop=url&titles=$(enc "$file")" | python3 -c "
import sys,json
try:
    d=json.load(sys.stdin)
    for p in d['query']['pages'].values():
        ii=p.get('imageinfo')
        if ii: print(ii[0]['url']); break
except Exception:
    pass
")
  [ -z "$url" ] && { echo "SKIP $sigla — imageinfo vazio"; continue; }
  ext="${url##*.}"; ext=$(echo "${ext%%\?*}" | tr '[:upper:]' '[:lower:]')
  case "$ext" in svg|png|jpg|jpeg|gif|webp) : ;; *) ext="png" ;; esac
  tmp="/tmp/logo3-$sigla.$ext"
  curl -sL --max-time 25 -A "DemocraciaBR-logo-import/1.0" -o "$tmp" "$url" || { echo "SKIP $sigla — download"; continue; }
  sz=$(stat -c%s "$tmp" 2>/dev/null || echo 0)
  [ "$sz" -lt 300 ] && { echo "SKIP $sigla — minúsculo"; continue; }
  key="parties/$sigla/logo.$ext"
  if mc cp "$tmp" "$BUCKET/$key" >/dev/null 2>&1; then
    echo "OK   $sigla → $key ($sz bytes) [$file]"
    echo "UPDATE party SET logo_url = '$MEDIA_BASE/$key' WHERE org_id = '$ORG' AND sigla = '$sigla';" >> "$OUT_SQL"
  else echo "FAIL $sigla — mc cp"; fi
  rm -f "$tmp"
done
echo "=== SQL ==="; cat "$OUT_SQL"

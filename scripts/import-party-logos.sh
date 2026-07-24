#!/usr/bin/env bash
# Importa logos de partidos da Wikipedia PT (imagem do infobox) pro MinIO e
# emite o SQL de logo_url. Roda NA VM de produção (tem mc alias dsoc + IPv6).
set -uo pipefail

BUCKET="dsoc/dsoc-media"
MEDIA_BASE="https://democracia.social.br/media"
OUT_SQL="/tmp/party-logos.sql"
: > "$OUT_SQL"

# sigla | título exato do artigo na pt.wikipedia
declare -a PARTIES=(
  "PT|Partido dos Trabalhadores"
  "MDB|Movimento Democrático Brasileiro"
  "PSDB|Partido da Social Democracia Brasileira"
  "PDT|Partido Democrático Trabalhista"
  "PCdoB|Partido Comunista do Brasil"
  "NOVO|Partido Novo"
  "PSOL|Partido Socialismo e Liberdade"
  "PV|Partido Verde (Brasil)"
  "REDE|Rede Sustentabilidade"
  "PP|Progressistas"
  "PL|Partido Liberal (2006)"
  "PSB|Partido Socialista Brasileiro"
  "REPUBLICANOS|Republicanos (partido político)"
  "PODE|Podemos (Brasil)"
  "UNIÃO|União Brasil"
  "SOLIDARIEDADE|Solidariedade (partido político)"
  "AVANTE|Avante (partido político)"
  "CIDADANIA|Cidadania (partido político)"
)

ORG="11111111-1111-1111-1111-111111111111"

for entry in "${PARTIES[@]}"; do
  sigla="${entry%%|*}"
  title="${entry#*|}"
  # API: imagem original do infobox (o logo), seguindo redirects.
  api="https://pt.wikipedia.org/w/api.php?action=query&format=json&prop=pageimages&piprop=original&redirects=1&titles=$(python3 -c "import urllib.parse,sys;print(urllib.parse.quote(sys.argv[1]))" "$title")"
  url=$(curl -s --max-time 15 "$api" | python3 -c "
import sys,json
try:
    d=json.load(sys.stdin)
    pages=d['query']['pages']
    for p in pages.values():
        src=p.get('original',{}).get('source')
        if src: print(src)
except Exception:
    pass
")
  if [ -z "$url" ]; then
    echo "SKIP $sigla — sem imagem ($title)"
    continue
  fi
  ext="${url##*.}"; ext="${ext%%\?*}"
  case "$ext" in svg|png|jpg|jpeg|gif|webp) : ;; *) ext="png" ;; esac
  tmp="/tmp/logo-$sigla.$ext"
  if ! curl -s --max-time 20 -A "DemocraciaBR-logo-import/1.0" -o "$tmp" "$url"; then
    echo "SKIP $sigla — download falhou"; continue
  fi
  sz=$(stat -c%s "$tmp" 2>/dev/null || echo 0)
  if [ "$sz" -lt 300 ]; then echo "SKIP $sigla — arquivo minúsculo ($sz)"; continue; fi
  key="parties/$sigla/logo.$ext"
  if mc cp "$tmp" "$BUCKET/$key" >/dev/null 2>&1; then
    echo "OK   $sigla → $key ($sz bytes)"
    echo "UPDATE party SET logo_url = '$MEDIA_BASE/$key' WHERE org_id = '$ORG' AND sigla = '$sigla';" >> "$OUT_SQL"
  else
    echo "FAIL $sigla — mc cp falhou"
  fi
  rm -f "$tmp"
done

echo "=== SQL gerado em $OUT_SQL ==="
cat "$OUT_SQL"

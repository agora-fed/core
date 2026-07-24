#!/usr/bin/env bash
# 2a passada: logos via Wikidata P154 (propriedade "logo image") → Commons
# Special:FilePath. Mais confiável que pageimages pros partidos que faltaram.
set -uo pipefail

BUCKET="dsoc/dsoc-media"
MEDIA_BASE="https://democracia.social.br/media"
ORG="11111111-1111-1111-1111-111111111111"
OUT_SQL="/tmp/party-logos-2.sql"
: > "$OUT_SQL"

# sigla | título do artigo na pt.wikipedia (resolve pro Wikidata via sitelink)
declare -a PARTIES=(
  "PSD|Partido Social Democrático (2011)"
  "UNIÃO|União Brasil"
  "PL|Partido Liberal (2006)"
  "REPUBLICANOS|Republicanos (partido político)"
  "PSB|Partido Socialista Brasileiro"
  "PODE|Podemos (Brasil)"
  "PODEMOS|Podemos (Brasil)"
  "AVANTE|Avante (partido político)"
  "PRD|Partido Renovação Democrática"
  "SOLIDARIEDADE|Solidariedade (partido político)"
  "CIDADANIA|Cidadania (partido político)"
  "REDE|Rede Sustentabilidade"
)

for entry in "${PARTIES[@]}"; do
  sigla="${entry%%|*}"
  title="${entry#*|}"
  enc=$(python3 -c "import urllib.parse,sys;print(urllib.parse.quote(sys.argv[1]))" "$title")
  # Wikidata: entidade pelo sitelink ptwiki → claim P154 (logo).
  fname=$(curl -s --max-time 15 "https://www.wikidata.org/w/api.php?action=wbgetentities&sites=ptwiki&titles=$enc&props=claims&format=json" | python3 -c "
import sys,json
try:
    d=json.load(sys.stdin); ents=d.get('entities',{})
    for e in ents.values():
        c=e.get('claims',{}).get('P154')
        if c:
            print(c[0]['mainsnak']['datavalue']['value']); break
except Exception:
    pass
")
  if [ -z "$fname" ]; then echo "SKIP $sigla — sem P154 ($title)"; continue; fi
  ext="${fname##*.}"; ext=$(echo "$ext" | tr '[:upper:]' '[:lower:]')
  case "$ext" in svg|png|jpg|jpeg|gif|webp) : ;; *) ext="png" ;; esac
  enc_f=$(python3 -c "import urllib.parse,sys;print(urllib.parse.quote(sys.argv[1].replace(' ','_')))" "$fname")
  tmp="/tmp/logo2-$sigla.$ext"
  if ! curl -sL --max-time 25 -A "DemocraciaBR-logo-import/1.0" -o "$tmp" "https://commons.wikimedia.org/wiki/Special:FilePath/$enc_f"; then
    echo "SKIP $sigla — download falhou"; continue
  fi
  sz=$(stat -c%s "$tmp" 2>/dev/null || echo 0)
  if [ "$sz" -lt 300 ]; then echo "SKIP $sigla — minúsculo ($sz)"; continue; fi
  key="parties/$sigla/logo.$ext"
  if mc cp "$tmp" "$BUCKET/$key" >/dev/null 2>&1; then
    echo "OK   $sigla → $key ($sz bytes) [$fname]"
    echo "UPDATE party SET logo_url = '$MEDIA_BASE/$key' WHERE org_id = '$ORG' AND sigla = '$sigla';" >> "$OUT_SQL"
  else
    echo "FAIL $sigla — mc cp"
  fi
  rm -f "$tmp"
done
echo "=== SQL ==="; cat "$OUT_SQL"

#!/usr/bin/env bash
# Backup diário do banco de produção (0.42.0). Cumpre a promessa de
# privacidade.astro: dump diário CRIPTOGRAFADO, retenção 30 dias.
#
# Roda como o usuário `postgres` (peer auth no socket local) via systemd timer.
# pg_dump -Fc (formato custom, já comprimido) → openssl AES-256 → arquivo .enc.
#
# Restaurar:
#   openssl enc -d -aes-256-cbc -pbkdf2 -pass file:/etc/dsoc/backup.key \
#     -in <arquivo>.dump.enc | pg_restore -d democracia_social --clean --if-exists
set -euo pipefail

DB="democracia_social"
DIR="/var/backups/dsoc"
KEY="/etc/dsoc/backup.key"
RETENTION_DAYS=30

mkdir -p "$DIR"
ts="$(date -u +%Y%m%d-%H%M%S)"
out="$DIR/${DB}-${ts}.dump.enc"

# Dump → cifra num pipe (o plaintext nunca toca o disco).
pg_dump -Fc "$DB" \
  | openssl enc -aes-256-cbc -pbkdf2 -salt -pass "file:$KEY" -out "$out"

# Sanidade: arquivo não-vazio.
if [ ! -s "$out" ]; then
  echo "backup FALHOU: arquivo vazio $out" >&2
  rm -f "$out"
  exit 1
fi

# Retenção: apaga os mais antigos que RETENTION_DAYS.
find "$DIR" -name "${DB}-*.dump.enc" -type f -mtime +"$RETENTION_DAYS" -delete

echo "backup OK: $out ($(stat -c%s "$out") bytes)"

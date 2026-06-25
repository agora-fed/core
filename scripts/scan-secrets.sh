#!/usr/bin/env bash
# Reject committed secrets. PLAN.md principle 8: secrets only in .config/settings.env (gitignored).
set -euo pipefail
cd "$(dirname "$0")/.."

# Patterns that must never appear in tracked files.
patterns='(FORGEJO_TOKEN=[A-Za-z0-9]{20,}|-----BEGIN [A-Z ]*PRIVATE KEY-----|AKIA[0-9A-Z]{16}|password\s*=\s*["'\''][^"'\'' ]{6,})'

# Only scan files git would track (respect .gitignore).
files="$(git ls-files 2>/dev/null || find . -type f -not -path './target/*' -not -path './.git/*')"
hits=0
for f in $files; do
  case "$f" in
    *settings.env.example|*scan-secrets.sh) continue ;;
  esac
  if grep -EqI "$patterns" "$f" 2>/dev/null; then
    echo "POSSIBLE SECRET in $f"
    hits=$((hits+1))
  fi
done

if [[ "$hits" -gt 0 ]]; then
  echo "FAILED: $hits possible secret(s) in tracked files."
  exit 1
fi
echo "OK: no secrets detected in tracked files."

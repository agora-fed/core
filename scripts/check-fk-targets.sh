#!/usr/bin/env bash
# Cross-crate FKs may target ONLY core identity tables (org, citizen, mandate) or a table created in
# the SAME migration file (a same-crate self/intra reference). Enforces crate isolation at the schema
# level (ARCHITECTURE.md section 3) without false-flagging legitimate intra-crate references.
set -euo pipefail
cd "$(dirname "$0")/.."
core='org|citizen|mandate'
violations=0
for f in migrations/*.sql; do
  [[ "$f" == *0001_baseline.sql ]] && continue
  # tables this migration defines (allowed self-reference targets)
  own=$(grep -ioE 'CREATE TABLE (IF NOT EXISTS )?[a-z_]+' "$f" | awk '{print $NF}' | tr '\n' '|' | sed 's/|$//')
  allowed="$core${own:+|$own}"
  while read -r tgt; do
    [[ -z "$tgt" ]] && continue
    if ! [[ "$tgt" =~ ^($allowed)$ ]]; then
      echo "FK VIOLATION in $f: REFERENCES $tgt (allowed: $core or a table created in this file)"
      violations=$((violations+1))
    fi
  done < <(grep -ioE 'REFERENCES[[:space:]]+[a-z_]+' "$f" | awk '{print $2}')
done
if [[ "$violations" -gt 0 ]]; then echo "FAILED: $violations cross-crate FK violation(s)."; exit 1; fi
echo "OK: cross-crate FK targets valid."

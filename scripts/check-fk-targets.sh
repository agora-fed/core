#!/usr/bin/env bash
# Cross-crate FKs may target ONLY core identity tables (org, citizen, mandate). Enforces the
# crate-isolation rule at the schema level (ARCHITECTURE.md section 3).
set -euo pipefail
cd "$(dirname "$0")/.."
allowed='org|citizen|mandate'
violations=0
while IFS= read -r ref; do
  file="${ref%%:*}"
  target=$(echo "$ref" | grep -oiE 'REFERENCES[[:space:]]+[a-z_]+' | awk '{print $2}' | head -1)
  # skip the baseline (defines the core identity tables itself)
  [[ "$file" == *0001_baseline.sql ]] && continue
  if [[ -n "$target" ]] && ! [[ "$target" =~ ^($allowed)$ ]]; then
    # allow self-references to a table the same migration file defines
    slug=$(basename "$file" | sed -E 's@[0-9]+_([a-z]+)_.*@\1@')
    if ! [[ "$target" == ${slug}* ]]; then
      echo "FK VIOLATION in $file: REFERENCES $target (only org/citizen/mandate or own tables allowed)"
      violations=$((violations+1))
    fi
  fi
done < <(grep -rinE 'REFERENCES[[:space:]]+[a-z_]+' migrations/ 2>/dev/null || true)
if [[ "$violations" -gt 0 ]]; then echo "FAILED: $violations cross-crate FK violation(s)."; exit 1; fi
echo "OK: cross-crate FK targets valid."

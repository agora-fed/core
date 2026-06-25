#!/usr/bin/env bash
# Every workspace member must opt into the shared lints so clippy -D warnings / unsafe-forbid apply.
set -euo pipefail
cd "$(dirname "$0")/.."
missing=0
while IFS= read -r manifest; do
  # skip the virtual workspace root and non-member dirs
  grep -q '^\[package\]' "$manifest" || continue
  if ! grep -q '^\[lints\]' "$manifest"; then
    echo "MISSING [lints] workspace = true in $manifest"
    missing=$((missing+1))
  fi
done < <(find crates tests -name Cargo.toml 2>/dev/null)
if [[ "$missing" -gt 0 ]]; then echo "FAILED: $missing manifest(s) missing [lints]."; exit 1; fi
echo "OK: all members opt into workspace lints."

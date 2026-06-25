#!/usr/bin/env bash
# Fail the build if a component/space crate path-depends on another component/space crate.
# Enforces PLAN.md: crates talk only via core traits, the event bus, and the gateway.
set -euo pipefail
cd "$(dirname "$0")/.."

violations=0
# Component/space crates may depend only on dsoc-core, dsoc-db, dsoc-api-contract.
allowed='dsoc-core|dsoc-db|dsoc-api-contract'
while IFS= read -r manifest; do
  crate_dir="$(dirname "$manifest")"
  case "$crate_dir" in
    */components/*|*/spaces/*) ;;
    *) continue ;;
  esac
  # Extract path-dependencies on other dsoc-* crates.
  while IFS= read -r dep; do
    if [[ "$dep" =~ ^dsoc- ]] && ! [[ "$dep" =~ ^($allowed)$ ]]; then
      echo "BOUNDARY VIOLATION: $crate_dir depends on $dep (only core/db/api-contract allowed)"
      violations=$((violations+1))
    fi
  done < <(grep -oE '^dsoc-[a-z-]+' "$manifest" || true)
done < <(find crates/components crates/spaces -name Cargo.toml)

if [[ "$violations" -gt 0 ]]; then
  echo "FAILED: $violations cross-crate boundary violation(s)."
  exit 1
fi
echo "OK: no cross-crate boundary violations."

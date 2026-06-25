#!/usr/bin/env bash
# Fail on duplicate migration number prefixes in the shared migrations/ dir (see REGISTRY.md).
set -euo pipefail
cd "$(dirname "$0")/.."
dupes=$(ls migrations/*.sql 2>/dev/null | sed -E 's@.*/([0-9]+)_.*@\1@' | sort | uniq -d || true)
if [[ -n "$dupes" ]]; then
  echo "DUPLICATE migration numbers: $dupes"
  exit 1
fi
echo "OK: migration numbers unique."

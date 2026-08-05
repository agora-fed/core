#!/usr/bin/env bash
# Web type-safety RATCHET (svelte-check over .svelte/.astro/.ts).
#
# Astro's build does NOT typecheck island code — a `res.ok` on a type without
# `ok` sails through `npm run build` and explodes in production (branding
# panel incident, 2026-08-05). svelte-check catches that class in seconds.
#
# The codebase carries pre-existing type errors, so this is a RATCHET, not an
# aspiration: MAX_ERRORS is the measured floor and may only go DOWN. Lower it
# in the same PR that fixes errors; raising it is a regression and needs an
# ADR-grade justification.
set -euo pipefail
cd "$(dirname "$0")/../web"

MAX_ERRORS=105   # measured 2026-08-05 (105 errors / 42 files)

errors="$(npx svelte-check --threshold error --output human 2>/dev/null \
  | grep -cE '^Error' || true)"
# Fallback for machine output variants: parse the summary line.
if [[ "$errors" -eq 0 ]]; then
  errors="$(npx svelte-check --threshold error 2>/dev/null \
    | grep -oE 'COMPLETED [0-9]+ FILES [0-9]+ ERRORS' \
    | grep -oE '[0-9]+ ERRORS' | grep -oE '[0-9]+' || echo 0)"
fi

echo "svelte-check errors: $errors (ratchet floor: $MAX_ERRORS)"
if (( errors > MAX_ERRORS )); then
  echo "FAILED: new type errors introduced ($errors > $MAX_ERRORS)."
  echo "Fix them — or, if you fixed some and the count DROPPED, lower MAX_ERRORS instead."
  exit 1
fi
if (( errors < MAX_ERRORS )); then
  echo "NOTE: count dropped below the floor — lower MAX_ERRORS to $errors in this PR."
fi
echo "OK: web type ratchet holds."

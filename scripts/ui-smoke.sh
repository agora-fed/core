#!/usr/bin/env bash
# Smoke UI pós-deploy (issue #36 / R6.2). Roda a suíte Playwright contra um alvo.
# Reusa o playwright de web/node_modules (não instala nada novo).
#
#   BASE_URL=https://democracia.social.br DSOC_SESSION=<cookie> scripts/ui-smoke.sh
#
# Sem DSOC_SESSION → só checagens públicas. Sai != 0 se qualquer checagem falhar,
# então serve de gate no CI/pós-deploy.
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO/web"
if [ ! -d node_modules/playwright ]; then
  echo "playwright ausente em web/node_modules — rode 'npm ci' em web/ primeiro." >&2
  exit 2
fi
# Garante o navegador (headless shell) — no-op se já instalado.
npx playwright install chromium --only-shell >/dev/null 2>&1 || true
exec node "$REPO/web/tests/smoke.mjs"
